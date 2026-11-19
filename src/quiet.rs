//! Keeping other update notices quiet.
//!
//! Once updates install themselves, two things CachyOS ships get in the way:
//!
//! - cachy-update: a tray icon and a daily "N updates available" check. This
//!   tool has its own icon, and the updates happen by themselves anyway.
//! - The "Reboot recommended" notice of cachyos-hooks. It pops up the moment a
//!   kernel is unpacked, in the middle of an unattended update, and reads as
//!   an invitation to cut the update in half.
//!
//! Both are off while automatic updates are on, unless CachyUpdate or
//! RebootNotification turns them back on. Each is undone the standard way:
//!
//! - The cachy-update timer is a user unit: disabled per user (and globally if
//!   it was enabled that way), enabled again per user.
//! - Its tray starts from /etc/xdg/autostart. A file of the same name in the
//!   user's own autostart folder with Hidden=true hides it for that user.
//! - pacman takes a hook in /etc/pacman.d/hooks over one of the same name in
//!   /usr/share/libalpm/hooks, and a link to /dev/null there turns it off.

use crate::config::Config;
use crate::users::{self, User};
use crate::{log, proc};
use std::path::Path;
use std::process::Command;

const CU_TIMER: &str = "arch-update.timer";
const CU_UNIT: &str = "/usr/lib/systemd/user/arch-update.timer";
const CU_GLOBAL: &str = "/etc/systemd/user/timers.target.wants/arch-update.timer";
const CU_TRAY: &str = "arch-update-tray.desktop";
const CU_TRAY_SYSTEM: &str = "/etc/xdg/autostart/arch-update-tray.desktop";
const CU_MARK: &str = "# Hidden by cachy-auto-update. Turn on \"Also run cachy-update\" to undo.";
const CU_TRAY_PATTERN: &str = "arch-update.*--tray|arch-update-tray";

const REBOOT_HOOK_SYSTEM: &str = "/usr/share/libalpm/hooks/cachyos-reboot-required.hook";
const REBOOT_HOOK_OVERRIDE: &str = "/etc/pacman.d/hooks/cachyos-reboot-required.hook";

fn exists(p: &str) -> bool {
    Path::new(p).exists()
}

fn is_link(p: &str) -> bool {
    std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink())
}

fn bus_up(user: &User) -> bool {
    Path::new(&format!("/run/user/{}/bus", user.uid)).exists()
}

/// Runs a command as the user, the plain way (runuser without the session
/// environment), for file work in their home.
fn as_owner(user: &User) -> Command {
    let mut c = Command::new("runuser");
    c.args(["-u", &user.name, "--"]);
    c
}

/// Makes the machine match the settings. Root only. `only_off` is for the
/// hourly run: it only switches things off and costs a few file checks.
/// Switching back on happens where a person changed a setting.
pub fn apply(cfg: &Config, only_off: bool) {
    if !proc::is_root() {
        return;
    }
    if cfg.enabled && !cfg.cachy_update {
        cachy_update_off();
    } else if !only_off {
        cachy_update_on();
    }
    if cfg.enabled && !cfg.reboot_notify {
        reboot_hook_off();
    } else if !only_off {
        reboot_hook_on();
    }
}

fn cachy_update_off() {
    if !exists(CU_UNIT) && !exists(CU_TRAY_SYSTEM) {
        return;
    }
    let mut global = false;
    if is_link(CU_GLOBAL) {
        proc::quiet(Command::new("systemctl").args(["--global", "disable", CU_TIMER]));
        log::info("Turned off cachy-update's update check for all users");
        global = true;
    }
    for user in users::human_users() {
        let wants = format!("{}/.config/systemd/user/timers.target.wants/{CU_TIMER}", user.home);
        if is_link(&wants) {
            if bus_up(&user) {
                proc::quiet(users::as_user(&user, "systemctl").args(["--user", "disable", "--now", CU_TIMER]));
            } else {
                proc::quiet(as_owner(&user).args(["rm", "-f", &wants]));
            }
            log::info(format!("Turned off cachy-update's update check for {}", user.name));
        } else if global && bus_up(&user) {
            // Was on for everyone until a moment ago: stop the running one.
            proc::quiet(users::as_user(&user, "systemctl").args(["--user", "stop", CU_TIMER]));
        }

        // The tray icon. A file the user made themselves is left alone.
        if !exists(CU_TRAY_SYSTEM) || !Path::new(&user.home).is_dir() {
            continue;
        }
        let file = format!("{}/.config/autostart/{CU_TRAY}", user.home);
        if std::fs::symlink_metadata(&file).is_ok() {
            continue;
        }
        let ok = proc::quiet(as_owner(&user).args([
            "sh",
            "-c",
            "mkdir -p \"${1%/*}\" && printf '%s\\n' \"$2\" '[Desktop Entry]' 'Hidden=true' > \"$1\"",
            "sh",
            &file,
            CU_MARK,
        ]));
        if !ok {
            continue;
        }
        proc::quiet(Command::new("pkill").args(["-u", &user.uid.to_string(), "-f", CU_TRAY_PATTERN]));
        log::info(format!("Hid cachy-update's tray icon for {}", user.name));
    }
}

fn cachy_update_on() {
    for user in users::human_users() {
        let file = format!("{}/.config/autostart/{CU_TRAY}", user.home);
        let ours = std::fs::read_to_string(&file).is_ok_and(|t| t.lines().any(|l| l == CU_MARK));
        if ours {
            proc::quiet(as_owner(&user).args(["rm", "-f", &file]));
        }
    }
    if !exists(CU_UNIT) {
        return;
    }
    for user in users::session_users() {
        proc::quiet(users::as_user(&user, "systemctl").args(["--user", "enable", "--now", CU_TIMER]));
        if !exists(CU_TRAY_SYSTEM) {
            continue;
        }
        if proc::quiet(Command::new("pgrep").args(["-u", &user.uid.to_string(), "-f", CU_TRAY_PATTERN])) {
            continue;
        }
        proc::quiet(users::as_user(&user, "systemd-run").args([
            "--user",
            "--quiet",
            "--collect",
            "--unit=arch-update-tray",
            "arch-update",
            "--tray",
        ]));
    }
}

fn reboot_hook_off() {
    if !Path::new(REBOOT_HOOK_SYSTEM).is_file() || std::fs::symlink_metadata(REBOOT_HOOK_OVERRIDE).is_ok() {
        return;
    }
    if let Some(dir) = Path::new(REBOOT_HOOK_OVERRIDE).parent()
        && std::fs::create_dir_all(dir).is_ok()
        && std::os::unix::fs::symlink("/dev/null", REBOOT_HOOK_OVERRIDE).is_ok()
    {
        log::info("Turned off CachyOS's restart notice");
    }
}

fn reboot_hook_on() {
    // Only the link this tool makes. A real file there is somebody's own hook.
    if std::fs::read_link(REBOOT_HOOK_OVERRIDE).is_ok_and(|t| t == Path::new("/dev/null")) {
        let _ = std::fs::remove_file(REBOOT_HOOK_OVERRIDE);
    }
}
