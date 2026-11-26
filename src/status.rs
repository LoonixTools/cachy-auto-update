//! What the CLI, the window and the tray icon show: the state the update run
//! wrote down, and wording shared between them.

use crate::config::Config;
use crate::i18n::{fill, tr_in};
use crate::paths::{TIMER, paths};
use crate::{proc, state};
use std::time::Duration;

/// How the last run ended.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum LastResult {
    None,
    Ok,
    Failed,
    Interrupted,
    Cancelled,
}

#[derive(Clone, Debug)]
pub struct Status {
    pub last_run: u64,
    pub last_success: u64,
    pub result: LastResult,
    /// Repository, AUR, Flatpak, AppImage.
    pub counts: [u64; 4],
    pub reboot: bool,
    /// AutoReboot restarts the computer once nobody is logged in.
    pub restart_planned: bool,
    pub held: Vec<String>,
}

impl Status {
    pub fn read() -> Self {
        let counts: Vec<u64> =
            state::read_or("last_counts", "0 0 0 0").split_whitespace().map(|n| n.parse().unwrap_or(0)).collect();
        let c = |i: usize| counts.get(i).copied().unwrap_or(0);
        Status {
            last_run: state::number("last_run"),
            last_success: state::number("last_success"),
            result: match state::read_or("last_result", "").as_str() {
                "ok" => LastResult::Ok,
                "failed" => LastResult::Failed,
                "interrupted" => LastResult::Interrupted,
                "cancelled" => LastResult::Cancelled,
                _ => LastResult::None,
            },
            counts: [c(0), c(1), c(2), c(3)],
            // Until the restart: the run cannot see it happen.
            reboot: state::read_or("reboot_needed", "0") == "1" && crate::pkg::pacman::reboot_needed(),
            restart_planned: crate::reboot::pending(),
            held: state::read_or("held_back", "").split_whitespace().map(String::from).collect(),
        }
    }

    pub fn total(&self) -> u64 {
        self.counts.iter().sum()
    }
}

/// The pid of a running update. A run killed before it could clean up does
/// not count.
pub fn running() -> Option<i32> {
    let pid: i32 = std::fs::read_to_string(paths().running()).ok()?.trim().parse().ok()?;
    std::path::Path::new(&format!("/proc/{pid}")).exists().then_some(pid)
}

/// How far a running update is, as the run writes it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunProgress {
    pub percent: u32,
    /// A msgid.
    pub label: String,
    pub done: u64,
    pub total: u64,
    pub paused: bool,
    pub detail: String,
}

pub fn run_progress() -> Option<RunProgress> {
    let text = std::fs::read_to_string(paths().progress()).ok()?;
    let l: Vec<&str> = text.lines().collect();
    let num = |i: usize| l.get(i).and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(0);
    Some(RunProgress {
        percent: num(0).min(100) as u32,
        label: l.get(1).unwrap_or(&"").to_string(),
        done: num(2),
        total: num(3),
        paused: l.get(4) == Some(&"1"),
        detail: l.get(5).unwrap_or(&"").to_string(),
    })
}

/// When the next automatic update happens.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Next {
    Unknown,
    NotScheduled,
    At(u64),
}

/// The timer ticks hourly, but a tick only updates once UpdateInterval has
/// passed since the last successful update. So that time wins, unless it is
/// past: then the next tick does it.
pub fn next_update(cfg: &Config, last_success: u64) -> Next {
    if !proc::have("systemctl") {
        return Next::Unknown;
    }
    if !cfg.enabled || !proc::quiet(std::process::Command::new("systemctl").args(["is-enabled", "--quiet", TIMER])) {
        return Next::NotScheduled;
    }
    let tick = proc::run("systemctl", &["show", TIMER, "--property=NextElapseUSecRealtime", "--value"], Duration::from_secs(10))
        .and_then(|o| o.stdout.trim().parse::<u64>().ok())
        .map(|us| us / 1_000_000)
        .unwrap_or(0);
    let due = if last_success > 0 { last_success + crate::conditions::interval_needed(cfg) } else { 0 };
    let at = if due > tick {
        due
    } else if tick > 0 {
        tick
    } else {
        return Next::Unknown;
    };
    Next::At(crate::conditions::next_in_hours(cfg, at))
}

/// UpdateHours' values, as the window and `status` name them.
pub fn hours_text(locale: &str, v: &str) -> String {
    match crate::config::Hours::parse(v) {
        Ok(None) => tr_in(locale, "Any time").into(),
        Ok(Some(h)) => fill(tr_in(locale, "Between %s and %s"), &[&format!("{:02}:00", h.from), &format!("{:02}:00", h.until)]),
        Err(_) => v.into(),
    }
}

/// AutoReboot's values, as the window and `status` name them.
pub fn reboot_text(locale: &str, mode: crate::config::Reboot) -> &'static str {
    use crate::config::Reboot;
    match mode {
        Reboot::Never => tr_in(locale, "Never"),
        Reboot::Needed => tr_in(locale, "After a kernel update"),
        Reboot::Always => tr_in(locale, "After every update"),
    }
}

/// "5 minutes ago". 0 is "never".
pub fn time_ago(locale: &str, ts: u64) -> String {
    if ts == 0 {
        return tr_in(locale, "never").into();
    }
    let d = state::now().saturating_sub(ts);
    match d {
        0..60 => tr_in(locale, "just now").into(),
        60..120 => tr_in(locale, "1 minute ago").into(),
        120..3600 => fill(tr_in(locale, "%d minutes ago"), &[&(d / 60)]),
        3600..7200 => tr_in(locale, "1 hour ago").into(),
        7200..86400 => fill(tr_in(locale, "%d hours ago"), &[&(d / 3600)]),
        86400..172800 => tr_in(locale, "1 day ago").into(),
        _ => fill(tr_in(locale, "%d days ago"), &[&(d / 86400)]),
    }
}

/// "every day", "every 2 weeks". The same length can be written more than
/// one way: 7d is a week.
pub fn interval_text(locale: &str, v: &str) -> String {
    let Some(secs) = crate::config::duration_seconds(v) else { return v.into() };
    let (n, unit) = if secs % 604800 == 0 {
        (secs / 604800, 'w')
    } else if secs % 86400 == 0 {
        (secs / 86400, 'd')
    } else if secs % 3600 == 0 {
        (secs / 3600, 'h')
    } else if secs % 60 == 0 {
        (secs / 60, 'm')
    } else {
        return v.into();
    };
    let (one, many) = match unit {
        'm' => ("every minute", "every %d minutes"),
        'h' => ("every hour", "every %d hours"),
        'd' => ("every day", "every %d days"),
        _ => ("every week", "every %d weeks"),
    };
    if n == 1 { tr_in(locale, one).into() } else { fill(tr_in(locale, many), &[&n]) }
}

/// A local date and time through strftime, in the locale LC_TIME names.
/// Call `init_time_locale` once first.
pub fn strftime(ts: u64, format: &str) -> String {
    format_time(ts, format, None)
}

/// strftime in `locale`, or in the one LC_TIME names.
fn format_time(ts: u64, format: &str, locale: Option<libc::locale_t>) -> String {
    let t = ts as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    let Ok(fmt) = std::ffi::CString::new(format) else { return String::new() };
    let mut buf = [0u8; 128];
    let (out, len, f) = (buf.as_mut_ptr().cast(), buf.len(), fmt.as_ptr());
    let n = match locale {
        Some(l) => unsafe { libc::strftime_l(out, len, f, &tm, l) },
        None => unsafe { libc::strftime(out, len, f, &tm) },
    };
    String::from_utf8_lossy(&buf[..n]).into_owned()
}

/// How the window shows dates.
enum Dates {
    /// In the locale of the environment.
    Env,
    /// In a locale of the language picked in the window.
    Picked(libc::locale_t),
    /// No locale of that language is installed: in numbers only, so the
    /// weekday is not in another language.
    Numbers,
}

// The locale is only used and freed with the lock held.
unsafe impl Send for Dates {}

static DATES: std::sync::Mutex<Dates> = std::sync::Mutex::new(Dates::Env);

/// Shows dates in `lang` ("es", "pt_BR", ...) from now on, or in the locale
/// of the environment again with `None`.
pub fn set_date_language(lang: Option<&str>) {
    let base = |l: &str| l.split(['_', '.', '@']).next().unwrap_or("").to_string();
    let env = ["LC_ALL", "LC_TIME", "LANG"].iter().find_map(|v| std::env::var(v).ok().filter(|v| !v.is_empty()));
    let env = env.map(|e| if e == "C" || e.starts_with("C.") || e == "POSIX" { "en".into() } else { base(&e) });
    let dates = match lang {
        None => Dates::Env,
        // Keeps the region of the environment, es_MX for "es".
        Some(l) if env.as_deref() == Some(base(l).as_str()) => Dates::Env,
        Some(l) => date_locales(l)
            .into_iter()
            .find_map(|name| {
                let name = std::ffi::CString::new(name).ok()?;
                let loc = unsafe { libc::newlocale(libc::LC_TIME_MASK, name.as_ptr(), std::ptr::null_mut()) };
                (!loc.is_null()).then_some(loc)
            })
            .map_or(Dates::Numbers, Dates::Picked),
    };
    let mut d = DATES.lock().unwrap_or_else(|e| e.into_inner());
    if let Dates::Picked(old) = std::mem::replace(&mut *d, dates) {
        unsafe { libc::freelocale(old) };
    }
}

/// Locales to try for `lang`, most likely first.
fn date_locales(lang: &str) -> Vec<String> {
    let full = match lang {
        "de" => "de_DE",
        "en" => "en_US",
        "es" => "es_ES",
        "fr" => "fr_FR",
        "it" => "it_IT",
        "ja" => "ja_JP",
        "ko" => "ko_KR",
        "ru" => "ru_RU",
        "pl" => "pl_PL",
        "tr" => "tr_TR",
        "nl" => "nl_NL",
        "uk" => "uk_UA",
        "cs" => "cs_CZ",
        "sv" => "sv_SE",
        "id" => "id_ID",
        "vi" => "vi_VN",
        "hu" => "hu_HU",
        "ro" => "ro_RO",
        l => l,
    };
    let mut out = vec![format!("{full}.UTF-8")];
    if lang == "en" {
        out.push("C.UTF-8".into());
    }
    out
}

/// The weekday, date and time, for the window.
pub fn date_time(ts: u64) -> String {
    match *DATES.lock().unwrap_or_else(|e| e.into_inner()) {
        Dates::Env => strftime(ts, "%a %x %H:%M"),
        Dates::Picked(l) => format_time(ts, "%a %x %H:%M", Some(l)),
        Dates::Numbers => strftime(ts, "%Y-%m-%d %H:%M"),
    }
}

/// Dates follow the locale the program was started in.
pub fn init_time_locale() {
    unsafe {
        libc::setlocale(libc::LC_TIME, c"".as_ptr());
    }
}
