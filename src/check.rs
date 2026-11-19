//! Looking for updates without installing them, for the tray icon and the
//! window. Runs as the user.
//!
//! Each look syncs a private copy of the package databases and asks the AUR,
//! so the callers do it rarely.

use crate::config::Config;
use crate::paths::VERSION;
use crate::{proc, state};
use std::collections::{BTreeMap, HashSet};
use std::time::Duration;

fn count_repo(skip: &HashSet<String>) -> Option<u64> {
    if !proc::have("checkupdates") {
        return None;
    }
    let out = proc::run("checkupdates", &["--nocolor"], Duration::from_secs(300))?;
    if !matches!(out.status, Some(0 | 2)) {
        return None;
    }
    Some(out.stdout.lines().filter_map(|l| l.split_whitespace().next()).filter(|n| !skip.contains(*n)).count() as u64)
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Asks the AUR directly rather than through paru or yay. While one of those
/// runs, the update waits for it, and this check must never hold an update up.
fn count_aur(skip: &HashSet<String>) -> Option<u64> {
    let out = proc::run("pacman", &["-Qm"], Duration::from_secs(60))?;
    if !matches!(out.status, Some(0 | 1)) {
        return None;
    }
    let installed: BTreeMap<String, String> = out
        .stdout
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            Some((it.next()?.to_string(), it.next()?.to_string()))
        })
        .filter(|(n, _)| !skip.contains(n) && crate::config::is_package_name(n))
        .collect();
    if installed.is_empty() {
        return Some(0);
    }
    let names: Vec<&String> = installed.keys().collect();
    let mut count = 0;
    for chunk in names.chunks(80) {
        let query: Vec<String> = chunk.iter().map(|n| format!("arg[]={}", urlencode(n))).collect();
        let url = format!("https://aur.archlinux.org/rpc/v5/info?{}", query.join("&"));
        let reply = proc::run(
            "curl",
            &["-fsS", "--max-time", "30", "-A", &format!("cachy-auto-update/{VERSION}"), &url],
            Duration::from_secs(40),
        )?;
        if !reply.ok() {
            return None;
        }
        let json: serde_json::Value = serde_json::from_str(&reply.stdout).ok()?;
        for pkg in json.get("results")?.as_array()? {
            let (Some(name), Some(version)) = (pkg.get("Name").and_then(|v| v.as_str()), pkg.get("Version").and_then(|v| v.as_str()))
            else {
                continue;
            };
            let Some(have) = installed.get(name) else { continue };
            if have == version {
                continue;
            }
            // Only newer counts. A -git package built today is ahead of the AUR.
            if proc::run("vercmp", &[have, version], Duration::from_secs(10)).is_some_and(|o| o.stdout.trim().starts_with('-')) {
                count += 1;
            }
        }
    }
    Some(count)
}

fn count_flatpak() -> Option<u64> {
    if !proc::have("flatpak") {
        return None;
    }
    let out = proc::run("flatpak", &["remote-ls", "--updates", "--columns=application"], Duration::from_secs(300))?;
    if !out.ok() {
        return None;
    }
    let apps: HashSet<&str> = out.stdout.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    Some(apps.len() as u64)
}

/// How many updates wait, or None when that is unknown (offline). Ignored
/// and held packages are left out: an update would not install them.
pub fn count_updates(cfg: &Config) -> Option<u64> {
    let mut skip: HashSet<String> = cfg.ignore_pkg.iter().cloned().collect();
    skip.extend(state::read_or("held_back", "").split_whitespace().map(String::from));
    let mut counts = vec![count_repo(&skip)];
    if cfg.aur {
        counts.push(count_aur(&skip));
    }
    if cfg.flatpak {
        counts.push(count_flatpak());
    }
    let known: Vec<u64> = counts.into_iter().flatten().collect();
    (!known.is_empty()).then(|| known.iter().sum())
}
