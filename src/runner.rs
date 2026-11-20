//! One unattended update pass (`cachy-auto-update run`).
//!
//! Started by cachy-auto-update.service as root. The timer fires hourly and
//! this decides whether anything should happen; that is what makes waiting
//! free. Not running is the normal outcome most of the time, never an error.

use crate::config::Config;
use crate::control::Control;
use crate::notify::{self, Note, Urgency};
use crate::paths::{PRETTY, paths, self_exe};
use crate::pkg::{aur, flatpak, pacman};
use crate::progress::{Outcome, Progress};
use crate::reboot::Then;
use crate::{conditions, locks, log, proc, quiet, state};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::sync::atomic::{AtomicI32, Ordering};

static SIGNAL_PIPE: AtomicI32 = AtomicI32::new(-1);

extern "C" fn on_signal(sig: libc::c_int) {
    let fd = SIGNAL_PIPE.load(Ordering::Relaxed);
    if fd >= 0 {
        let b = sig as u8;
        unsafe {
            libc::write(fd, (&b as *const u8).cast(), 1);
        }
    }
}

/// An interruption is recorded rather than leaving the last run's verdict
/// behind, which could keep reporting a problem from hours ago on a machine
/// that is up to date. pacman makes its commit itself uninterruptible, so
/// only our own bookkeeping is at risk here.
fn watch_signals(progress: Progress) {
    let Ok((mut r, w)) = log::pipe() else { return };
    use std::os::fd::IntoRawFd;
    SIGNAL_PIPE.store(w.into_raw_fd(), Ordering::Relaxed);
    for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        unsafe {
            libc::signal(sig, on_signal as *const () as libc::sighandler_t);
        }
    }
    std::thread::spawn(move || {
        use std::io::Read;
        let mut b = [0u8; 1];
        if r.read_exact(&mut b).is_err() {
            return;
        }
        let name = match b[0] as i32 {
            libc::SIGINT => "SIGINT",
            libc::SIGTERM => "SIGTERM",
            _ => "SIGHUP",
        };
        log::error(format!("Update run interrupted ({name})"));
        // First: an entry whose owner just vanishes is reported as a crash.
        progress.end(Outcome::Failed, "The update was stopped before it finished.");
        let _ = state::write("last_result", "interrupted");
        crate::reboot::announce(None);
        let _ = std::fs::remove_file(paths().running());
        std::process::exit(130);
    });
}

fn note<'a>(urgency: Urgency, linger: bool, title: &'a str, body: &'a str, args: Vec<String>) -> Note<'a> {
    Note { urgency, linger, title, body, args }
}

pub fn main(args: &[String]) -> i32 {
    let (mut force, mut dry_run, mut by_hand) = (false, false, false);
    let mut then = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--force" => force = true,
            // Somebody clicked Update now and waits for it: like --force, and
            // the bar shows whatever NotifyOnStart and MinimizeProgress say.
            "--now" => {
                force = true;
                by_hand = true;
            }
            "--dry-run" => dry_run = true,
            "--debug" => log::set_debug(true),
            // Update and shut down, or Update and restart. Somebody asked for
            // it, so like --force.
            "--then" => then = it.next().and_then(|t| Then::parse(t)),
            s if s.starts_with("--then=") => then = Then::parse(&s[7..]),
            _ => {}
        }
        if a.starts_with("--then") {
            if then.is_none() {
                eprintln!("cachy-auto-update run: --then takes poweroff or reboot");
                return 2;
            }
            force = true;
        }
    }
    if !proc::is_root() {
        eprintln!("cachy-auto-update run: must run as root");
        return 2;
    }

    // A neutral locale, so a run started by hand behaves as one started by
    // the timer. pacman's failures are sorted by its output, which would be
    // German on a German system. Text for people names its own locale.
    unsafe {
        std::env::set_var("LC_ALL", "C");
        std::env::set_var("LANGUAGE", "");
    }

    // Again under an inhibitor, so a suspend or shutdown cannot land in the
    // middle of a pacman transaction and leave the database half written.
    if std::env::var_os("CAU_INHIBITED").is_none() && proc::have("systemd-inhibit") {
        unsafe { std::env::set_var("CAU_INHIBITED", "1") };
        let err = Command::new("systemd-inhibit")
            .args(["--what=sleep:shutdown", "--mode=block", &format!("--who={PRETTY}"), "--why=Applying system updates", "--"])
            .arg(self_exe())
            .arg("run")
            .args(args)
            .exec();
        log::warn(format!("Could not take an inhibitor: {err}"));
    }

    let cfg = Config::load();
    log::open();
    let progress = Progress::new();
    watch_signals(progress.clone());

    let defer = |why: &str| {
        log::info(format!("Deferred: {why}"));
        0
    };

    let Some(_lock) = locks::acquire_run_lock() else {
        log::debug("Another run is already in progress");
        return 0;
    };

    if !force {
        if !cfg.enabled {
            log::debug("Automatic updates are disabled");
            return 0;
        }
        // Every hour, so a user who logs in later, or a cachy-update that was
        // installed later, is quiet too.
        quiet::apply(&cfg, true);
        // A restart that waited for everybody to log out. No update starts
        // with one on its way.
        if crate::reboot::tick(&cfg) {
            return 0;
        }
        if !conditions::update_due(&cfg) {
            log::debug(format!("Not due yet (interval {})", cfg.interval));
            return 0;
        }
        if !conditions::in_hours(&cfg) {
            log::debug(format!("Outside the hours for updates ({})", cfg.get("UpdateHours")));
            return 0;
        }
    }

    let _ = state::write("last_run", &state::now().to_string());

    if !force {
        if let Err(why) = conditions::power_ok(&cfg) {
            return defer(&why);
        }
        if cfg.skip_gaming
            && let Some(why) = conditions::busy()
        {
            return defer(&why);
        }
    }

    // A lock left by a power cut during an update. Cleared before the busy
    // check, or every future run would wait for it forever.
    if locks::recover_stale_lock() {
        notify::notify(
            &cfg,
            note(
                Urgency::Normal,
                false,
                "Finishing an interrupted update",
                "The last update was cut short, most likely because the machine was switched off. It is being finished now.",
                vec![],
            ),
        );
    }

    // Checked even with --force: going on would only end in a lock error.
    if let Some(why) = locks::package_manager_busy() {
        if locks::track_stale_lock() {
            log::warn("pacman's database lock appears to be stale");
            notify::notify(
                &cfg,
                note(
                    Urgency::Normal,
                    true,
                    "Package database locked",
                    "pacman's lock file looks left over from an interrupted update. Updates are paused until it is cleared.",
                    vec![],
                ),
            );
        }
        return defer(&why);
    }
    state::clear("stale_lock_count");

    if dry_run {
        return self::dry_run(&cfg);
    }

    log::reset_runlog();
    log::info("Starting update run");

    // Tells the tray icon and the window that an update runs, and what
    // follows it. That first, so they see both at once.
    crate::reboot::announce(then);
    let _ = std::fs::write(paths().running(), format!("{}\n", std::process::id()));
    let _ = std::fs::set_permissions(paths().running(), std::fs::Permissions::from_mode(0o644));

    // The bar, told which steps this run takes. Only those count, so a
    // machine with no Flatpaks does not sit at 85% for the last second.
    let mut steps = vec!["resolve", "download", "repo", "hooks"];
    if cfg.aur {
        steps.push("aur");
    }
    if cfg.flatpak {
        steps.extend(["flatpak", "userflatpak"]);
    }
    if cfg.appimage {
        steps.push("appimage");
    }
    steps.push("cleanup");
    let control = Control::new();
    progress.begin(&cfg, by_hand, &steps, &control);
    progress.forecast_start(&cfg);
    progress.tail_start();

    let mut failed = false;
    let pac = pacman::update(&cfg, &progress, &control);
    failed |= !pac.ok;

    // Between the steps the run waits while paused, and stops once cancelled.
    let aur = if cfg.aur && control.wait(&progress) { aur::update(&cfg, &progress, &control) } else { aur::Outcome::default() };
    let mut fp_count = 0;
    if cfg.flatpak && control.wait(&progress) {
        let (ok, n) = flatpak::update(&cfg, &progress, &control);
        failed |= !ok;
        fp_count = n;
    }
    let ai_count =
        if cfg.appimage && control.wait(&progress) { crate::pkg::appimage::update(&progress, &control) } else { 0 };
    if control.wait(&progress) {
        pacman::cleanup(&cfg, &progress);
        aur::clean_caches(&cfg);
    }

    // A step stopped by Cancel returns success, so failed still means
    // something really went wrong.
    let cancelled = control.cancelled();
    progress.end(
        if cancelled {
            Outcome::Cancelled
        } else if failed {
            Outcome::Failed
        } else {
            Outcome::Ok
        },
        "",
    );
    control.stop();

    let pacnew = pacman::pacnew_count();
    if pacnew > 0 {
        log::info(format!("{pacnew} .pacnew file(s) present; left untouched"));
    }

    let total = pac.count + aur.count + fp_count + ai_count;
    let _ = state::write("last_counts", &format!("{} {} {fp_count} {ai_count}", pac.count, aur.count));

    if failed {
        let _ = state::write("last_result", "failed");
        log::error("Update run finished with errors");
        if cfg.notify_error {
            notify::notify_tagged(
                &cfg,
                Some("run"),
                true,
                note(
                    Urgency::Critical,
                    true,
                    "Update failed",
                    "Something went wrong while updating. Run 'cachy-auto-update log' for details.",
                    vec![],
                ),
            );
        }
    } else if cancelled {
        // Not a success either, so the next hourly check runs it again.
        let _ = state::write("last_result", "cancelled");
        log::info("Update run cancelled from the desktop");
        notify::notify_tagged(
            &cfg,
            Some("run"),
            false,
            note(
                Urgency::Normal,
                false,
                "Update stopped",
                "You can switch off the computer now. The update will run again later.",
                vec![],
            ),
        );
    } else {
        let _ = state::write("last_result", "ok");
        let _ = state::write("last_success", &state::now().to_string());
        log::info(format!("Update run finished successfully ({total} item(s) updated)"));
        // Not when the computer goes off right after.
        if total > 0 && cfg.notify_success && then.is_none() {
            notify::notify_tagged(
                &cfg,
                Some("run"),
                true,
                note(Urgency::Low, false, "System updated", "%d updates were installed.", vec![total.to_string()]),
            );
        }
    }

    // Packages skipped so the rest could go through. Only a change is
    // announced: a blocker waiting on an upstream fix would otherwise bring
    // the same message every day.
    let prev = state::read_or("held_back", "");
    let held: Vec<String> = pac.held.iter().chain(aur.held.iter()).cloned().collect();
    let held = held.join(" ");
    if !held.is_empty() {
        let _ = state::write("held_back", &held);
        log::warn(format!("Held back: {held}"));
        if held != prev && cfg.notify_error {
            notify::notify(
                &cfg,
                note(
                    Urgency::Normal,
                    true,
                    "Some packages were held back",
                    "%s could not be updated and was skipped. Everything else is up to date.",
                    vec![held.clone()],
                ),
            );
        }
    } else if !cancelled {
        // A cancelled run may not have got that far, so it knows nothing new.
        state::clear("held_back");
    }

    // Recorded, on purpose not announced: the running kernel loses its
    // modules as soon as pacman unpacks the new one, so this turns true in the
    // middle of a run. The window and `status` say it instead.
    let kernel = pacman::reboot_needed();
    if kernel {
        let _ = state::write("reboot_needed", "1");
        log::info("A kernel update needs a restart");
    } else {
        let _ = state::write("reboot_needed", "0");
    }

    // The state stays readable for `status`, the window and the tray icon.
    // Only the plain files: the build account's home lives in here too.
    let dir = &paths().state;
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755));
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            if e.file_type().is_ok_and(|t| t.is_file()) {
                let _ = std::fs::set_permissions(e.path(), std::fs::Permissions::from_mode(0o644));
            }
        }
    }
    let _ = std::fs::remove_file(paths().running());

    // Last, with everything written down: shut down or restart as asked, even
    // after a problem, since nobody may be there. Not after Cancel. Else
    // AutoReboot.
    match then {
        Some(t) if !cancelled => crate::reboot::then(t),
        _ => {
            crate::reboot::announce(None);
            crate::reboot::after_run(&cfg, !failed && !cancelled, total, kernel);
        }
    }
    i32::from(failed)
}

fn dry_run(cfg: &Config) -> i32 {
    println!("Would update on this machine:");
    match pacman::pending() {
        Some(list) => {
            println!("  repository packages: {}", list.len());
            for l in list {
                println!("    {l}");
            }
        }
        None => println!("  repository packages: 0"),
    }
    match cfg.aur.then(|| aur::pending(cfg)).flatten() {
        Some((n, helper)) => println!("  AUR packages:        {n} ({helper})"),
        None => println!("  AUR packages:        skipped"),
    }
    if cfg.flatpak && proc::have("flatpak") {
        println!("  system Flatpaks:     {}", flatpak::pending_system());
    } else {
        println!("  system Flatpaks:     skipped");
    }
    println!("  AppImages:           {}", if cfg.appimage { "checked via Gear Lever" } else { "skipped" });
    0
}
