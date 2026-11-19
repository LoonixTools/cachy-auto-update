//! AutoReboot: the computer restarts by itself after an update.
//!
//! Off by default. Only after a run that went through, and only while nobody
//! is logged in to the desktop: a restart would end their session and lose
//! what is not saved. Until then the restart waits, and each hourly tick
//! looks again. People on a terminal or over SSH get a message first.

use crate::config::{Config, Reboot};
use crate::i18n::{system_locale, trf_in};
use crate::{locks, log, proc, state, users};
use std::process::Command;

/// Minutes between the message and the restart.
const DELAY_MINUTES: u32 = 2;

fn boot_id() -> String {
    std::fs::read_to_string("/proc/sys/kernel/random/boot_id").map(|s| s.trim().to_string()).unwrap_or_default()
}

/// A restart waits in this boot. The file holds the boot it was planned in,
/// so a restart by any other means clears it.
pub fn pending() -> bool {
    state::read("reboot_pending").is_some_and(|id| !id.is_empty() && id == boot_id())
}

/// Whether a planned restart still happens by itself. Only the hourly tick
/// does it, and that only looks while automatic updates are on.
pub fn will_happen(cfg: &Config, planned: bool) -> bool {
    planned && cfg.enabled && cfg.auto_reboot != Reboot::Never
}

/// Whether a run that went through asks for a restart. A restart that was
/// still waiting stays planned.
pub fn wanted(mode: Reboot, installed: u64, kernel: bool, waiting: bool) -> bool {
    match mode {
        Reboot::Never => false,
        Reboot::Needed => kernel,
        Reboot::Always => installed > 0 || kernel || waiting,
    }
}

/// After a run. Only one that went through can plan a restart; anything
/// else drops a planned one.
pub fn after_run(cfg: &Config, ok: bool, installed: u64, kernel: bool) {
    if !ok || !wanted(cfg.auto_reboot, installed, kernel, pending()) {
        state::clear("reboot_pending");
        return;
    }
    let _ = state::write("reboot_pending", &boot_id());
    if let Err(why) = try_now() {
        log::info(format!("A restart is planned. It waits: {why}"));
    }
}

/// On each hourly tick: restarts if one waits and nothing stands in the way.
/// True when the restart is on its way, so no update starts.
pub fn tick(cfg: &Config) -> bool {
    if !pending() {
        return false;
    }
    if cfg.auto_reboot == Reboot::Never || state::read_or("last_result", "") != "ok" {
        state::clear("reboot_pending");
        return false;
    }
    // Every hour while somebody is logged in: not worth a line in the log.
    try_now().inspect_err(|why| log::debug(format!("Restart waits: {why}"))).is_ok()
}

/// Asks logind to restart in a few minutes, unless somebody is at the
/// desktop or uses pacman. logind tells everybody on a terminal, and lets
/// nobody log in until then. Err says why it waits.
fn try_now() -> Result<(), String> {
    if let Some(user) = users::session_users().first() {
        return Err(format!("{} is logged in", user.name));
    }
    if let Some(why) = locks::package_manager_busy() {
        return Err(why);
    }
    let locale = system_locale().unwrap_or_else(|| "C".into());
    let message = trf_in(
        &locale,
        "This computer restarts in %d minutes to finish an update. To stop it, run: sudo shutdown -c",
        &[&DELAY_MINUTES],
    );
    let when = format!("+{DELAY_MINUTES}");
    if !proc::quiet(Command::new("shutdown").args(["-r", &when, &message])) {
        return Err("shutdown failed".into());
    }
    log::info(format!("Restarting in {DELAY_MINUTES} minutes to finish the update"));
    // Planned now. Whoever stops it with `shutdown -c` is not asked again.
    state::clear("reboot_pending");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_what_the_setting_asks_for() {
        assert!(!wanted(Reboot::Never, 10, true, true));
        assert!(wanted(Reboot::Needed, 0, true, false));
        assert!(!wanted(Reboot::Needed, 10, false, false));
        assert!(wanted(Reboot::Always, 3, false, false));
        assert!(!wanted(Reboot::Always, 0, false, false));
        assert!(wanted(Reboot::Always, 0, false, true));
    }
}
