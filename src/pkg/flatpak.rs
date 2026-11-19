//! Flatpak.
//!
//! System installs are updated directly as root. Flatpak's polkit rule only
//! allows an active, local subject in the wheel group, none of which a service
//! is; as root, polkit is never asked. Per-user installs live in the user's
//! home and are updated as that user.

use crate::config::Config;
use crate::control::{Control, Kind};
use crate::progress::{FLATPAK_SECONDS, Progress};
use crate::users::{self, User};
use crate::{log, proc};
use std::process::Command;
use std::time::Duration;

fn count_lines(out: Option<proc::Output>) -> u64 {
    out.filter(|o| o.ok()).map(|o| o.stdout.lines().filter(|l| !l.trim().is_empty()).count() as u64).unwrap_or(0)
}

pub fn pending_system() -> u64 {
    count_lines(proc::run("flatpak", &["remote-ls", "--system", "--updates", "--columns=application"], Duration::from_secs(300)))
}

fn pending_user(u: &User) -> u64 {
    let mut cmd = users::as_user(u, "flatpak");
    cmd.args(["remote-ls", "--user", "--updates", "--columns=application"]);
    count_lines(proc::output(&mut cmd, Duration::from_secs(300)))
}

fn has_user_install(u: &User) -> bool {
    std::path::Path::new(&u.home).join(".local/share/flatpak").is_dir()
}

/// System and user apps pending. Only for the bar.
pub fn forecast() -> (u64, u64) {
    let mine = users::human_users().iter().filter(|u| has_user_install(u)).map(pending_user).sum();
    (pending_system(), mine)
}

/// The next part on the bar: this many apps, from now. Flatpak prints
/// nothing countable, so the bar goes by the clock.
fn part(progress: &Progress, step: &'static str, n: u64) {
    progress.weigh(step, n * FLATPAK_SECONDS);
    progress.step(step, "Updating Flatpak apps", n);
    progress.tick_start(1, None);
}

/// A Flatpak command that Cancel and Pause can stop. After a pause it runs
/// again, and ostree keeps what it had fetched.
fn run(progress: &Progress, control: &Control, mk: &dyn Fn() -> Command) -> bool {
    loop {
        if control.run(progress, Kind::Flatpak, None, &mut mk(), None).is_some_and(|s| s.success()) {
            return true;
        }
        if control.stopped() && control.wait(progress) {
            continue;
        }
        return false;
    }
}

/// After a failed update: repair the install, then the caller tries once
/// more. A half-pulled object in the repo is the usual cause. False when
/// trying again is pointless.
fn repair(progress: &Progress, control: &Control, mut cmd: Command) -> bool {
    if control.cancelled() {
        return false;
    }
    log::info("Repairing the Flatpak installation and retrying");
    control.run(progress, Kind::Flatpak, None, &mut cmd, None);
    !control.cancelled()
}

/// A failure is reported but never stops the rest of the run. Returns
/// (ok, apps updated).
pub fn update(cfg: &Config, progress: &Progress, control: &Control) -> (bool, u64) {
    if !proc::have("flatpak") {
        progress.drop_steps(&["flatpak", "userflatpak"]);
        return (true, 0);
    }
    let (mut ok, mut count) = (true, 0);
    progress.step("flatpak", "Updating Flatpak apps", 0);

    // Fresh appstream data first, so remote-ls sees current versions.
    progress.tick_start(1, None);
    run(progress, control, &|| {
        let mut c = Command::new("flatpak");
        c.args(["update", "--appstream", "--system", "--noninteractive"]);
        c
    });
    progress.tick_stop();
    if control.cancelled() {
        return (true, count);
    }

    let system_update = || {
        let mut c = Command::new("flatpak");
        c.args(["update", "--system", "--noninteractive", "--assumeyes"]);
        c
    };
    let pending = pending_system();
    if pending > 0 {
        log::info(format!("Updating {pending} system Flatpak(s)"));
        part(progress, "flatpak", pending);
        let mut repair_cmd = Command::new("flatpak");
        repair_cmd.args(["repair", "--system"]);
        let done = run(progress, control, &system_update)
            || (repair(progress, control, repair_cmd) && run(progress, control, &system_update));
        progress.tick_stop();
        if done {
            count += pending;
            progress.item(pending, pending);
        } else if control.cancelled() {
            return (true, count);
        } else {
            log::warn("System Flatpak update failed");
            ok = false;
        }
    } else {
        log::info("No system Flatpak updates pending");
    }

    for user in users::human_users() {
        if control.cancelled() {
            return (true, count);
        }
        if !has_user_install(&user) {
            continue;
        }
        let n = pending_user(&user);
        if n == 0 {
            continue;
        }
        log::info(format!("Updating {n} user Flatpak(s) for {}", user.name));
        part(progress, "userflatpak", n);
        let update = || {
            let mut c = users::as_user(&user, "flatpak");
            c.args(["update", "--user", "--noninteractive", "--assumeyes"]);
            c
        };
        let mut repair_cmd = users::as_user(&user, "flatpak");
        repair_cmd.args(["repair", "--user"]);
        let done = run(progress, control, &update)
            || (repair(progress, control, repair_cmd) && run(progress, control, &update));
        progress.tick_stop();
        if done {
            count += n;
            progress.item(n, n);
        } else if control.cancelled() {
            return (true, count);
        } else {
            log::warn(format!("User Flatpak update failed for {}", user.name));
            ok = false;
        }
    }

    // Unused runtimes are Flatpak's orphans. Removing them removes installed
    // software, so it goes with RemoveOrphans, not CleanCache.
    if cfg.remove_orphans && !control.cancelled() {
        log::run_logged(Command::new("flatpak").args(["uninstall", "--system", "--unused", "--noninteractive", "--assumeyes"]));
    }
    (ok, count)
}
