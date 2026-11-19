//! Should we update right now?
//!
//! Every check may only say "not now". The timer ticks hourly, so waiting
//! costs nothing: the next tick tries again.

use crate::config::Config;
use crate::i18n::{tr, trf};
use crate::{proc, state, users};
use std::time::Duration;

fn read(path: &std::path::Path, name: &str) -> Option<String> {
    std::fs::read_to_string(path.join(name)).ok().map(|s| s.trim().to_string())
}

/// True on mains power. systemd-ac-power also says yes on a machine with no
/// battery and no adapter, which is right for a desktop.
pub fn on_ac() -> bool {
    if proc::have("systemd-ac-power") {
        return proc::quiet(&mut std::process::Command::new("systemd-ac-power"));
    }
    if let Ok(dir) = std::fs::read_dir("/sys/class/power_supply") {
        for ps in dir.flatten().map(|e| e.path()) {
            if read(&ps, "type").as_deref() == Some("Mains") && read(&ps, "online").as_deref() == Some("1") {
                return true;
            }
        }
    }
    // No adapter online: a machine without a battery is a desktop.
    battery_percent().is_none()
}

/// The average charge of the system batteries, or None without one. Mice and
/// headsets report batteries too; their scope is not System.
pub fn battery_percent() -> Option<i64> {
    let dir = std::fs::read_dir("/sys/class/power_supply").ok()?;
    let (mut sum, mut count) = (0, 0);
    for ps in dir.flatten().map(|e| e.path()) {
        if read(&ps, "type").as_deref() != Some("Battery") {
            continue;
        }
        if read(&ps, "scope").is_some_and(|s| s != "System") {
            continue;
        }
        if read(&ps, "present").is_some_and(|p| p != "1") {
            continue;
        }
        if let Some(cap) = read(&ps, "capacity").and_then(|c| c.parse::<i64>().ok()) {
            sum += cap;
            count += 1;
        }
    }
    (count > 0).then(|| sum / count)
}

/// RequireAC and MinBatteryPercent. Err holds the reason, translated.
pub fn power_ok(cfg: &Config) -> Result<(), String> {
    if on_ac() {
        return Ok(());
    }
    if cfg.require_ac {
        return Err(tr("running on battery").into());
    }
    match battery_percent() {
        Some(pct) if pct < cfg.min_battery => {
            Err(trf("battery at %d%%, below the %d%% threshold", &[&pct, &cfg.min_battery]))
        }
        _ => Ok(()),
    }
}

/// Process names that mean a game really runs. Not "steam" itself: it sits
/// in the tray all day on a gaming machine.
pub const GAME_PROCESSES: &[&str] = &[
    "gamescope", "wine", "wine64", "wineserver", "wine-preloader", "wine64-preloader", "umu-run", "proton", "reaper",
    "lutris", "lutris-wrapper", "heroic", "bottles", "retroarch", "dolphin-emu", "pcsx2-qt", "rpcs3", "cemu",
    "ryujinx", "ppsspp", "citra-qt", "duckstation-qt",
];

/// Asks GameMode, in each graphical session, how many games it tracks.
/// GameMode is optional; any failure means "don't know", not "busy".
fn gamemode_active() -> bool {
    if !proc::have("busctl") {
        return false;
    }
    for user in users::session_users() {
        let mut cmd = users::as_user(&user, "timeout");
        cmd.args([
            "5",
            "busctl",
            "--user",
            "--json=short",
            "get-property",
            "com.feralinteractive.GameMode",
            "/com/feralinteractive/GameMode",
            "com.feralinteractive.GameMode",
            "ClientCount",
        ]);
        let Some(out) = proc::output(&mut cmd, Duration::from_secs(10)) else { continue };
        if !out.ok() {
            continue;
        }
        let digits: String = out
            .stdout
            .split("\"data\"")
            .nth(1)
            .unwrap_or(&out.stdout)
            .chars()
            .skip_while(|c| !c.is_ascii_digit())
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if digits.parse::<u64>().is_ok_and(|n| n > 0) {
            return true;
        }
    }
    false
}

/// A blocking "idle" inhibitor: what fullscreen games and video players take.
fn idle_inhibited() -> bool {
    let Some(out) = proc::run("systemd-inhibit", &["--list"], Duration::from_secs(10)) else { return false };
    out.stdout.lines().any(|l| l.to_lowercase().contains("idle") && l.split_whitespace().last() == Some("block"))
}

/// Something runs that an update should not get in the way of. Returns the
/// reason, translated.
pub fn busy() -> Option<String> {
    if gamemode_active() {
        return Some(tr("a game is running (GameMode)").into());
    }
    let procs = proc::processes();
    if let Some(game) = GAME_PROCESSES.iter().find(|g| proc::running(&procs, g)) {
        return Some(trf("a game is running (%s)", &[game]));
    }
    if idle_inhibited() {
        return Some(tr("an application is blocking idle (fullscreen game or video)").into());
    }
    None
}

/// Whether enough time passed since the last successful update.
pub fn update_due(cfg: &Config) -> bool {
    let last = state::number("last_success");
    last == 0 || state::now().saturating_sub(last) >= interval_needed(cfg)
}

/// How long after an update the next one is due. With UpdateHours, up to
/// half a day less: the hourly check comes at a different minute each time,
/// and a daily update made at 04:20 would miss a 04:10 check the next day,
/// then the rest of the hours, and wait a whole day more.
pub fn interval_needed(cfg: &Config) -> u64 {
    let i = cfg.interval_seconds;
    if cfg.hours.is_some() { i - i.min(86400) / 2 } else { i }
}

/// Whether an update may start now, by UpdateHours.
pub fn in_hours(cfg: &Config) -> bool {
    cfg.hours.is_none_or(|h| h.contains(local(state::now()).tm_hour as u32))
}

/// The first moment from `ts` on at which an update may start.
pub fn next_in_hours(cfg: &Config, ts: u64) -> u64 {
    let Some(h) = cfg.hours else { return ts };
    let mut tm = local(ts);
    if h.contains(tm.tm_hour as u32) {
        return ts;
    }
    let start = |tm: &mut libc::tm| {
        (tm.tm_hour, tm.tm_min, tm.tm_sec, tm.tm_isdst) = (h.from as i32, 0, 0, -1);
        unsafe { libc::mktime(tm) as u64 }
    };
    let today = start(&mut tm);
    if today > ts {
        return today;
    }
    tm.tm_mday += 1;
    start(&mut tm)
}

fn local(ts: u64) -> libc::tm {
    let t = ts as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    tm
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Hours;
    use std::collections::HashMap;

    fn cfg(hours: &str, interval: &str) -> Config {
        Config::from_raw(HashMap::from([("UpdateHours".into(), hours.into()), ("UpdateInterval".into(), interval.into())]))
    }

    #[test]
    fn waits_for_the_next_start_of_the_hours() {
        let now = state::now();
        let hour = local(now).tm_hour as u32;
        // Hours that start an hour from now and so do not hold this one.
        let from = (hour + 1) % 24;
        let c = cfg(&format!("{from}-{}", (from + 2) % 24), "1d");
        assert!(!in_hours(&c));
        let next = next_in_hours(&c, now);
        let tm = local(next);
        assert!(next > now && next - now <= 3600 + 60);
        assert_eq!((tm.tm_hour as u32, tm.tm_min, tm.tm_sec), (from, 0, 0));
        // Inside the hours, and without any, it is now.
        assert_eq!(next_in_hours(&c, next + 60), next + 60);
        assert_eq!(next_in_hours(&cfg("any", "1d"), now), now);
        assert!(Hours::parse(&format!("{hour}-{}", (hour + 1) % 24)).unwrap().unwrap().contains(hour));
    }

    #[test]
    fn hours_let_a_run_come_a_little_early() {
        assert_eq!(interval_needed(&cfg("any", "1d")), 86400);
        assert_eq!(interval_needed(&cfg("3-6", "1d")), 43200);
        assert_eq!(interval_needed(&cfg("3-6", "1w")), 604800 - 43200);
    }
}
