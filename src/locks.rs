//! Staying out of the way of a person using pacman, yay or paru.
//!
//! The owner may run a package manager at any time and must never see a lock
//! error because of us. That only works one way: by never starting while
//! somebody else is in a transaction. So these checks run before anything is
//! touched, and saying no just waits for the next tick.

use crate::i18n::{tr, trf};
use crate::paths::{PACMAN_LOCK, paths};
use crate::{log, proc, state};
use std::os::fd::AsRawFd;
use std::path::Path;
use std::time::Duration;

/// Programs that take pacman's lock. checkupdates is not one: it works on a
/// private copy of the databases. Its own pacman is left out too, see
/// `harmless_pacman`.
pub const PACKAGE_MANAGERS: &[&str] = &[
    "pacman", "pacman-static", "yay", "paru", "pikaur", "aurman", "trizen", "pamac", "pamac-daemon",
    "pamac-manager", "pamac-tray", "octopi", "octopi-helper", "makepkg",
];

/// Held for as long as a run lasts, so two runs never overlap.
pub struct RunLock(#[allow(dead_code)] std::fs::File);

pub fn acquire_run_lock() -> Option<RunLock> {
    std::fs::create_dir_all(&paths().run).ok()?;
    let f = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(paths().lockfile()).ok()?;
    let r = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    (r == 0).then_some(RunLock(f))
}

/// The pids that have pacman's lock open. None when the lock is not there.
/// Without fuser that cannot be told; then it counts as held.
fn lock_holders() -> Option<Vec<i32>> {
    if !Path::new(PACMAN_LOCK).exists() {
        return None;
    }
    let Some(out) = proc::run("fuser", &[PACMAN_LOCK], Duration::from_secs(10)) else {
        return Some(vec![0]);
    };
    Some(out.stdout.split_whitespace().filter_map(|w| w.parse().ok()).collect())
}

/// Somebody else uses the package system right now. Returns the reason.
pub fn package_manager_busy() -> Option<String> {
    if Path::new(PACMAN_LOCK).exists() {
        return Some(tr("pacman's database is locked by another process").into());
    }
    let procs = proc::processes();
    PACKAGE_MANAGERS
        .iter()
        .find(|name| {
            proc::named(&procs, name).any(|p| !(name.starts_with("pacman") && harmless_pacman(&proc::args(p.pid))))
        })
        .map(|p| trf("%s is currently running", &[p]))
}

/// A pacman that never takes the system's lock: one that only reads (-Q, -T,
/// -V), or one on a private copy of the databases (--dbpath). The look for
/// updates runs both, through checkupdates, and must not hold an update up.
fn harmless_pacman(args: &[String]) -> bool {
    args.iter().any(|a| match a.as_str() {
        "-b" | "--dbpath" | "--query" | "--deptest" | "--version" => true,
        _ if a.starts_with("--dbpath=") => true,
        // The operation is the capital among the short flags: -Qm, -Syu.
        _ if a.starts_with('-') && !a.starts_with("--") => {
            matches!(a.chars().find(char::is_ascii_uppercase), Some('Q' | 'T' | 'V'))
        }
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use super::harmless_pacman;

    fn harmless(line: &str) -> bool {
        harmless_pacman(&line.split_whitespace().map(String::from).collect::<Vec<_>>())
    }

    #[test]
    fn only_lock_free_pacman_is_harmless() {
        // What checkupdates and the look for AUR updates run.
        assert!(harmless("-Sy --disable-sandbox-filesystem --dbpath /tmp/checkup-db-1000 --logfile /dev/null"));
        assert!(harmless("-Qu --dbpath /tmp/checkup-db-1000"));
        assert!(harmless("-Qm"));
        assert!(harmless("--query --info foo"));
        // A person installing or removing.
        assert!(!harmless("-Syu"));
        assert!(!harmless("-S --needed foo"));
        assert!(!harmless("-Rns foo"));
        assert!(!harmless("-U /var/cache/pacman/pkg/foo.pkg.tar.zst"));
        // Unreadable arguments count as busy.
        assert!(!harmless(""));
    }
}

/// True only when the lock surely belongs to nothing alive: older than this
/// boot, and held by no process. That is what a power cut during an update
/// leaves behind. A lock that is merely unheld within this boot is not
/// certain, so it is only reported, never removed.
fn lock_is_stale() -> bool {
    let Some(boot) = proc::boot_time() else { return false };
    let Ok(meta) = std::fs::metadata(PACMAN_LOCK) else { return false };
    let Some(mtime) = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()) else {
        return false;
    };
    mtime.as_secs() < boot && lock_holders().is_some_and(|h| h.is_empty())
}

/// Removes a lock that surely is abandoned, so an interrupted update can be
/// finished. Without this, one power cut stops every future update.
pub fn recover_stale_lock() -> bool {
    if !lock_is_stale() {
        return false;
    }
    log::warn("Found a pacman lock older than this boot. An update was cut short");
    if std::fs::remove_file(PACMAN_LOCK).is_err() {
        log::error("Could not remove the stale pacman lock");
        return false;
    }
    state::clear("stale_lock_count");
    log::info("Stale lock removed; the interrupted update will be finished now");
    true
}

/// How many runs in a row see an unheld lock before the user hears of it.
const STALE_LOCK_RUNS: u64 = 3;

/// A lock with no process behind it, made during this boot: a crashed pacman
/// rather than a power cut. Counted, and after a few runs the user is told.
pub fn track_stale_lock() -> bool {
    match lock_holders() {
        Some(h) if h.is_empty() => {
            let count = state::number("stale_lock_count") + 1;
            let _ = state::write("stale_lock_count", &count.to_string());
            count >= STALE_LOCK_RUNS
        }
        _ => {
            state::clear("stale_lock_count");
            false
        }
    }
}
