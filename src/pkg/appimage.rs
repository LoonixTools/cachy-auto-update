//! AppImages, by way of Gear Lever.
//!
//! AppImages have no package manager; Gear Lever tracks where each came from.
//! `--update --all -y` is the unattended way in, and it skips AppImages whose
//! app runs right now (we never pass --force).
//!
//! Only for users with a graphical session: Gear Lever is a GTK app and needs
//! the session's runtime folder. Nothing is lost by waiting: the next hourly
//! tick catches it once they log in.

use crate::control::{Control, Kind};
use crate::progress::Progress;
use crate::users::{self, User};
use crate::{log, proc};
use std::time::Duration;

const GEARLEVER: &str = "it.mijorus.gearlever";

/// How to start Gear Lever for that user, if they have it.
fn gearlever(u: &User) -> Option<Vec<&'static str>> {
    if users::user_has(u, "gearlever") {
        return Some(vec!["gearlever"]);
    }
    if proc::have("flatpak") && proc::quiet(users::as_user(u, "flatpak").args(["info", GEARLEVER])) {
        return Some(vec!["flatpak", "run", "--command=gearlever", GEARLEVER]);
    }
    None
}

/// Never fails the run: a GTK app with no window is best effort. Returns
/// how many AppImages were updated.
pub fn update(progress: &Progress, control: &Control) -> u64 {
    // Asked before the step shows: on most machines there is no Gear Lever,
    // and a step that only hands its share of the bar on is a jump.
    let found: Vec<(User, Vec<&str>)> =
        users::session_users().into_iter().filter_map(|u| gearlever(&u).map(|c| (u, c))).collect();
    if found.is_empty() {
        progress.drop_steps(&["appimage"]);
        return 0;
    }
    progress.step("appimage", "Updating AppImages", 0);
    progress.tick_start(1, None);
    let mut count = 0;
    for (user, cmd) in found {
        if control.cancelled() {
            break;
        }
        let mut list = users::as_user(&user, "timeout");
        list.arg("300").args(&cmd).arg("--list-updates");
        // "[Update available, <manager>]" marks each app in the plain listing.
        let n = proc::output(&mut list, Duration::from_secs(320))
            .map(|o| o.stdout.lines().filter(|l| l.contains("[Update available")).count() as u64)
            .unwrap_or(0);
        if n == 0 {
            log::info(format!("No AppImage updates pending for {}", user.name));
            continue;
        }
        log::info(format!("Updating {n} AppImage(s) for {}", user.name));
        // Gear Lever swaps the files in place, so Cancel and Pause wait for it.
        let mut up = users::as_user(&user, "timeout");
        up.arg("1800").args(&cmd).args(["--update", "--all", "-y"]);
        if control.run(progress, Kind::Other, None, &mut up, None).is_some_and(|s| s.success()) {
            count += n;
        } else if control.cancelled() {
            break;
        } else {
            log::warn(format!("AppImage update failed for {}", user.name));
        }
    }
    progress.tick_stop();
    count
}
