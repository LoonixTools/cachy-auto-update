//! Repository packages.
//!
//! The step is skipped when checkupdates finds nothing. That matters more
//! than it looks: checkupdates works on a private copy of the databases, so on
//! the common "nothing to do" day the real pacman lock is never taken and a
//! person using pacman is never in the way.

use crate::config::Config;
use crate::control::{Control, Kind};
use crate::notify::{self, Note, Urgency};
use crate::paths::paths;
use crate::progress::{Progress, pacing};
use crate::{log, proc, state};
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// What the repository step did.
#[derive(Default)]
pub struct Outcome {
    pub ok: bool,
    pub count: u64,
    /// Packages skipped so the rest of the upgrade could go through.
    pub held: Vec<String>,
}

/// Bytes per second downloads are taken to run at, until a run has measured
/// this machine's line.
const DEFAULT_RATE: u64 = 6_291_456;

fn rate() -> u64 {
    match state::number("download_rate") {
        0 => DEFAULT_RATE,
        n => n,
    }
}

const VERBS: &[&str] = &["upgrading", "installing", "reinstalling", "downgrading", "removing"];

/// A verb from VERBS followed by a package name somewhere in the line.
fn has_verb(line: &str) -> bool {
    VERBS.iter().any(|v| {
        line.match_indices(v).any(|(i, _)| {
            let rest = &line[i + v.len()..];
            rest.starts_with(' ') && rest[1..].chars().next().is_some_and(|c| !c.is_whitespace())
        })
    })
}

/// Whether pacman got past the downloads: it checks, runs hooks and installs
/// from here on, and must not be interrupted any more (see control.rs).
pub fn committed(text: &str) -> bool {
    const MARKS: &[&str] = &[
        "checking keyring",
        "checking keys in keyring",
        "checking package integrity",
        "loading package files",
        "checking for file conflicts",
        "checking available disk space",
        "pre-transaction hooks",
        "Processing package changes",
    ];
    text.lines().any(|l| MARKS.iter().any(|m| l.contains(m)) || has_verb(l))
}

/// The flags of every unattended pacman.
fn flags(cfg: &Config) -> Vec<String> {
    let mut f: Vec<String> = ["--noconfirm", "--color", "never", "--disable-download-timeout"].map(String::from).to_vec();
    // A bar is worth it while somebody watches; in the log it is noise.
    if !log::interactive() {
        f.push("--noprogressbar".into());
    }
    for p in &cfg.ignore_pkg {
        f.push("--ignore".into());
        f.push(p.clone());
    }
    f
}

/// Everything the bar needs from pacman's output, in one pass.
#[derive(Default, Debug)]
struct Watch {
    total: u64,
    size: u64,
    upgrade: bool,
    fetch: bool,
    started: u64,
    commit: bool,
    n: u64,
    counted: u64,
    outof: u64,
    hooks: bool,
    file: String,
    name: String,
}

/// Reads pacman's output so far. Matched in English on purpose: the run
/// forces LC_ALL=C so pacman stays parseable.
///
/// The database sync prints the same "downloading..." lines as the packages
/// do, so those count only after "Retrieving packages". In the transaction
/// pacman names each package as "upgrading glibc..." (no bar, every timer
/// run) or "( 12/218) upgrading glibc [##]" (with the bar).
fn watch(text: &str) -> Watch {
    let mut w = Watch::default();
    for line in text.lines() {
        if w.total == 0 && (line.starts_with("Packages (") || line.starts_with("Package (")) {
            let digits: String = line.split_whitespace().nth(1).unwrap_or("").chars().filter(char::is_ascii_digit).collect();
            w.total = digits.parse().unwrap_or(0);
        }
        if let Some(rest) = line.strip_prefix("Total Download Size:") {
            let mut it = rest.split_whitespace();
            let num: f64 = it.next().and_then(|n| n.parse().ok()).unwrap_or(0.0);
            let unit = match it.next() {
                Some("KiB") => 1024.0,
                Some("MiB") => 1_048_576.0,
                Some("GiB") => 1_073_741_824.0,
                _ => 1.0,
            };
            w.size = (num * unit) as u64;
        }
        if line.starts_with(":: Starting full system upgrade") {
            w.upgrade = true;
        }
        if line.starts_with(":: Retrieving packages") {
            w.fetch = true;
            continue;
        }
        if w.fetch && line.ends_with(" downloading...") {
            w.started += 1;
            w.file = line.split_whitespace().next().unwrap_or("").to_string();
        }
        if committed(line) {
            w.commit = true;
        }
        // "( 12/218) upgrading glibc" or "upgrading glibc..."
        let (counter, rest) = match line.strip_prefix('(').and_then(|r| r.split_once(") ")) {
            Some((c, rest)) if c.chars().all(|ch| ch == ' ' || ch == '/' || ch.is_ascii_digit()) => (Some(c), rest),
            _ => (None, line),
        };
        if let Some(verb) = VERBS.iter().find(|v| rest.starts_with(&format!("{v} ")))
            && let Some(name) = rest[verb.len() + 1..].split_whitespace().next()
        {
            w.n += 1;
            if let Some((a, b)) = counter.and_then(|c| c.split_once('/')) {
                w.counted = a.trim().parse().unwrap_or(0);
                w.outof = b.trim().parse().unwrap_or(0);
            }
            w.name = name.trim_end_matches("...").to_string();
        }
        if line.starts_with(":: Running post-transaction hooks") {
            w.hooks = true;
        }
    }
    w
}

/// The bare name in "glibc-2.40-1-x86_64": version, release and
/// architecture go.
fn bare(file: &str) -> &str {
    let parts: Vec<&str> = file.rsplitn(4, '-').collect();
    if parts.len() == 4 { parts[3] } else { file }
}

/// Bytes in a folder and below.
fn du(dir: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    rd.flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => du(&e.path()),
            Ok(_) => e.metadata().map(|m| m.len()).unwrap_or(0),
            Err(_) => 0,
        })
        .sum()
}

/// The folder pacman downloads into this time: download-* in the cache, made
/// since the run began. Old runs leave such folders behind.
fn download_dir(cache: &Path, since: std::time::SystemTime) -> Option<std::path::PathBuf> {
    std::fs::read_dir(cache).ok()?.flatten().find_map(|e| {
        let new = e.metadata().ok()?.modified().ok()? >= since;
        (e.file_name().to_string_lossy().starts_with("download-") && e.path().is_dir() && new).then(|| e.path())
    })
}

/// Feeds the bar by watching pacman work, through the four stretches a
/// pacman run has. Each is silent in its own way:
///
///   resolve   pacman works out the upgrade and prints nothing. On a large
///             backlog that is minutes. The bar goes by the clock.
///   download  pacman names each download as it starts and says nothing when
///             one is done, so the bar goes by the bytes that arrived.
///   repo      one line per package. The total comes from "Packages (218)".
///   hooks     the boot image, the caches. By the clock again.
///
/// The log is read again every second rather than through a pipe: one
/// reader for both the terminal and the timer is worth a second of delay.
fn watcher(progress: Progress, logfile: std::path::PathBuf, mut count: u64, stop: Arc<AtomicBool>) {
    #[derive(PartialEq)]
    enum Phase {
        Resolve,
        Download,
        Repo,
        Hooks,
    }
    let mut phase = Phase::Resolve;
    let (mut labelled, mut shown) = (false, String::new());
    let mut t0 = Instant::now();
    let cache = proc::run("pacman-conf", &["CacheDir"], Duration::from_secs(10))
        .and_then(|o| o.stdout.lines().next().map(|l| std::path::PathBuf::from(l.trim())));
    let rate = rate();
    let since = std::time::SystemTime::now() - Duration::from_secs(2);
    let (mut dir, mut got) = (None, 0u64);
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_secs(1));
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let text = std::fs::read(&logfile).map(|b| String::from_utf8_lossy(&b).replace('\r', "\n")).unwrap_or_default();
        let mut w = watch(&text);
        if w.total > 0 {
            count = w.total;
        }
        // Where pacman does carry a counter, believe it over the tally.
        if w.outof > 0 {
            count = w.outof;
            w.n = w.counted;
        }
        if w.hooks {
            if phase != Phase::Hooks {
                phase = Phase::Hooks;
                progress.step("hooks", "Finishing the installation", 0);
            }
            progress.pace(0, 1, 1);
            continue;
        }
        // Checking and unpacking. If nothing was downloaded (all in the cache,
        // normal after an interrupted run), the download step had no turn.
        if w.commit {
            if phase != Phase::Repo {
                // How fast this line really is, for the next run's bar.
                let secs = t0.elapsed().as_secs();
                if phase == Phase::Download && w.size > 0 && secs >= 5 {
                    let _ = state::write("download_rate", &((rate + w.size / secs) / 2).to_string());
                }
                phase = Phase::Repo;
                shown.clear();
                t0 = Instant::now();
                progress.step("repo", "Updating system packages", count);
            }
            // In fifths of a second: 30 for the checks, which go by the clock,
            // and 2 for each package.
            let pre = if w.n == 0 { pacing(t0.elapsed().as_secs(), 6) as u64 } else { 10000 };
            progress.at(3 * pre / 1000 + 2 * w.n, 30 + 2 * count);
            let now = format!("{} {}", w.n, w.name);
            if now != shown {
                shown = now;
                progress.count(w.n, count);
                if !w.name.is_empty() {
                    progress.detail("Package", &w.name);
                }
            }
            continue;
        }
        if w.fetch {
            if phase != Phase::Download {
                phase = Phase::Download;
                shown.clear();
                t0 = Instant::now();
                if w.size > 0 {
                    progress.weigh("download", w.size / rate + 2);
                }
                progress.step("download", "Downloading updates", count);
            }
            if dir.is_none()
                && let Some(c) = &cache
            {
                dir = download_dir(c, since);
            }
            if let Some(d) = &dir {
                got = got.max(du(d));
            }
            if w.size > 0 && got > 0 {
                progress.at(got, w.size);
            } else {
                progress.pace(0, 1, 1);
            }
            let now = w.started.to_string();
            if now != shown {
                shown = now;
                progress.count(w.started, count);
                if !w.file.is_empty() {
                    progress.detail("Package", bare(&w.file));
                }
            }
            continue;
        }
        // Still working out the upgrade. pacman marks where it stops syncing
        // and starts on the upgrade, and that half takes the minutes.
        if w.upgrade && !labelled {
            labelled = true;
            progress.step("resolve", "Preparing the update", 0);
        }
        progress.pace(0, 1, 1);
    }
}

/// The pending updates, one "name old -> new" per line. None means nothing
/// to do. Without pacman-contrib there is no cheap look ahead, so the list is
/// empty and pacman works it out.
pub fn pending() -> Option<Vec<String>> {
    if !proc::have("checkupdates") {
        return Some(Vec::new());
    }
    // Not in the cache folder, which belongs to the build account.
    let db = paths().state.join("checkupdates-db");
    let db = if std::fs::create_dir_all(&db).is_ok() { db } else { std::env::temp_dir().join("cachy-auto-update-checkupdates") };
    let out = proc::output(
        Command::new("checkupdates").arg("--nocolor").env("CHECKUPDATES_DB", db),
        Duration::from_secs(900),
    )?;
    let lines: Vec<String> = out.stdout.lines().filter(|l| !l.trim().is_empty()).map(String::from).collect();
    if out.status == Some(2) || lines.is_empty() {
        return None;
    }
    Some(lines)
}

/// Which kind of failure this was.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Failure {
    Conflict,
    Dependency,
    Keyring,
    Filesystem,
    Other,
}

fn classify(log: &str) -> Failure {
    let l = log.to_lowercase();
    if l.contains("are in conflict") || l.contains("unresolvable package conflicts") {
        Failure::Conflict
    } else if l.contains("could not satisfy dependencies")
        || l.contains("breaks dependency")
        || l.contains("unable to satisfy dependency")
    {
        Failure::Dependency
    } else if l.lines().any(|line| {
        (line.contains("signature from ")
            && (line.contains("is unknown trust") || line.contains("is marginal trust") || line.contains("is invalid")))
            || line.contains("invalid or corrupted package (pgp signature)")
            || (line.contains("key \"") && line.contains("\" is unknown"))
            || line.contains("keyring is not writable")
    }) {
        Failure::Keyring
    } else if l.contains("exists in filesystem") {
        Failure::Filesystem
    } else {
        Failure::Other
    }
}

/// The packages in the way of an otherwise fine upgrade:
///
///   :: removing libfoo breaks dependency 'libfoo' required by bar
///   :: unable to satisfy dependency 'foo' required by bar
///
/// In the first the package being removed is the one to keep; in the second
/// it is the package that cannot be installed.
fn blockers(log: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in log.lines() {
        if let Some(i) = line.find("removing ")
            && line[i..].contains(" breaks dependency")
        {
            if let Some(name) = line[i + 9..].split_whitespace().next() {
                out.push(name.to_string());
            }
        } else if line.contains("unable to satisfy dependency '")
            && let Some((_, by)) = line.split_once("' required by ")
            && let Some(name) = by.split_whitespace().next()
        {
            out.push(name.to_string());
        }
    }
    out.retain(|n| crate::config::is_package_name(n));
    out.sort();
    out.dedup();
    out
}

/// About how long the repository steps take, for the bar. Measured on real
/// runs: the checks before the first package take about six seconds, a
/// package installs in 0.4, the hooks after take half as long again, and
/// rebuilding the boot image adds half a minute. Eight megabytes a package
/// stands in until pacman says how big the download is.
fn weigh(progress: &Progress, n: u64, pending: &[String]) {
    if n == 0 {
        return;
    }
    let boot = pending.iter().any(|l| {
        let name = l.split_whitespace().next().unwrap_or("");
        ["linux", "mkinitcpio", "systemd", "nvidia"].iter().any(|p| name.starts_with(p)) || name.ends_with("-dkms")
    });
    progress.weigh("download", n * 8_388_608 / rate() + 2);
    progress.weigh("repo", 6 + n * 2 / 5);
    progress.weigh("hooks", 5 + n / 5 + if boot { 25 } else { 0 });
}

fn installed(pkg: &str) -> bool {
    proc::quiet(Command::new("pacman").args(["-Qq", pkg]))
}

/// The repository step. ok is false only on a failure a person needs to hear
/// about.
pub fn update(cfg: &Config, progress: &Progress, control: &Control) -> Outcome {
    // checkupdates goes first, and the bar says so rather than naming a step
    // that has not begun. The watcher takes over once pacman runs.
    progress.step("resolve", "Checking for updates", 0);
    progress.tick_start(1, None);
    let pending = pending();
    progress.tick_stop();
    progress.forecast();
    let Some(pending) = pending else {
        log::info("No repository updates pending");
        // None of the steps this would have led to happens, so the rest of
        // the run gets their share of the bar.
        progress.drop_steps(&["download", "repo", "hooks"]);
        return Outcome { ok: true, ..Default::default() };
    };
    let count = pending.len() as u64;
    if count > 0 {
        log::info(format!("Updating {count} repository package(s)"));
    } else {
        log::info("Running a full system upgrade (pending list unavailable)");
    }
    weigh(progress, count, &pending);

    // Clicked while checkupdates ran: no start notice for an update that is
    // paused or not going to happen.
    if !control.wait(progress) {
        return Outcome { ok: true, ..Default::default() };
    }

    // Said before the transaction starts. While an upgrade runs, a shutdown
    // is refused and the desktop answers with an untranslated polkit prompt
    // that never mentions updates.
    if cfg.notify_start {
        notify::notify_tagged(
            cfg,
            Some("run"),
            false,
            Note {
                urgency: Urgency::Normal,
                linger: false,
                title: "Installing updates",
                body: "%d packages are being updated. Please leave the computer switched on until this is done.",
                args: vec![count.to_string()],
            },
        );
    }

    let logfile = paths().run.join("pacman.log");
    let base = flags(cfg);
    // A loop rather than one retry: fixing one problem often uncovers the
    // next. Each remedy is applied once at most, so this always ends.
    let (mut extra, mut tried, mut held): (Vec<String>, Vec<Failure>, Vec<String>) = (vec![], vec![], vec![]);
    let mut attempt = 0;
    loop {
        let _ = std::fs::File::create(&logfile);
        let stop = Arc::new(AtomicBool::new(false));
        let watch = {
            let (p, l, s) = (progress.clone(), logfile.clone(), stop.clone());
            std::thread::spawn(move || watcher(p, l, count, s))
        };
        // Line-buffered: pacman writes to a file or a pipe here, so libc
        // would buffer 4 KB, about 160 packages, and the bar would sit still
        // and then leap.
        let mut cmd = if proc::have("stdbuf") {
            let mut c = Command::new("stdbuf");
            c.args(["-oL", "pacman"]);
            c
        } else {
            Command::new("pacman")
        };
        cmd.arg("-Syu").args(&base).args(&extra);
        let status = control.run(progress, Kind::Pacman, Some(&logfile), &mut cmd, Some(&logfile));
        stop.store(true, Ordering::Relaxed);
        let _ = watch.join();
        let text = std::fs::read(&logfile).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
        log::append_runlog(&text);

        if status.is_some_and(|s| s.success()) {
            for line in text.lines().filter(|l| l.starts_with("removing ") || l.starts_with("replacing ")) {
                log::info(format!("  {line}"));
            }
            let _ = std::fs::remove_file(&logfile);
            return Outcome { ok: true, count, held };
        }

        // Stopped from the desktop, not failed. After a pause the same try
        // runs again; what was downloaded is still in the cache.
        if status.is_none() || control.stopped() || control.cancelled() {
            if control.wait(progress) {
                continue;
            }
            log::info("pacman was stopped from the desktop");
            let _ = std::fs::remove_file(&logfile);
            return Outcome { ok: true, ..Default::default() };
        }

        let kind = classify(&text);
        log::warn(format!("pacman -Syu failed ({})", format!("{kind:?}").to_lowercase()));
        attempt += 1;
        if attempt > 3 || tried.contains(&kind) {
            break;
        }
        tried.push(kind);
        match kind {
            Failure::Keyring => {
                // A stale keyring is always safe to fix, and it blocks
                // everything else until it is.
                log::info("Refreshing keyrings and retrying");
                let rings: Vec<&str> =
                    ["archlinux-keyring", "cachyos-keyring"].into_iter().filter(|k| installed(k)).collect();
                if !rings.is_empty() {
                    let mut c = Command::new("pacman");
                    c.args(["-Sy", "--noconfirm", "--color", "never"]).args(&rings);
                    control.run(progress, Kind::Other, None, &mut c, None);
                }
            }
            Failure::Conflict => {
                // --noconfirm already says yes to "Replace X with Y?"; what it
                // declines is ":: X and Y are in conflict. Remove Y?". --ask is
                // pacman's question bitmask: 4 = CONFLICT_PKG, 16 = REMOVE_PKGS.
                if !cfg.resolve_conflicts {
                    log::error("Package conflict requires a decision (AutoResolveConflicts is off)");
                    break;
                }
                log::info("Resolving package conflicts automatically and retrying");
                extra.push("--ask=20".into());
            }
            Failure::Dependency => {
                // Something installed still needs a package the repos drop,
                // most often an AUR package that has not caught up. One stuck
                // package must not block every other update: hold it back.
                let b = blockers(&text);
                if b.is_empty() {
                    log::error("Dependency problem with no package to hold back");
                    break;
                }
                for p in &b {
                    extra.push("--ignore".into());
                    extra.push(p.clone());
                }
                log::warn(format!("Holding back {} and retrying without them", b.join(" ")));
                held = b;
            }
            Failure::Filesystem => {
                // Files in the way. --overwrite could clobber something put
                // there on purpose, so this stays a person's decision.
                log::error("Files on disk conflict with the update; manual review needed");
                break;
            }
            Failure::Other => {
                // Mostly a mirror that was down or a broken download. pacman
                // deletes a corrupt file itself. Wait, fetch fresh databases
                // and try again.
                log::info("Refreshing the package databases and retrying");
                std::thread::sleep(Duration::from_secs(15));
                extra.push("-y".into());
            }
        }
    }
    let _ = std::fs::remove_file(&logfile);
    Outcome::default()
}

/// The running kernel's module tree has no vmlinuz any more: the kernel was
/// replaced under it.
pub fn reboot_needed() -> bool {
    let mut u: libc::utsname = unsafe { std::mem::zeroed() };
    if unsafe { libc::uname(&mut u) } != 0 {
        return false;
    }
    let release = unsafe { std::ffi::CStr::from_ptr(u.release.as_ptr()) }.to_string_lossy().into_owned();
    !Path::new(&format!("/usr/lib/modules/{release}/vmlinuz")).is_file()
}

/// How many .pacnew files there are. They are never touched.
pub fn pacnew_count() -> usize {
    proc::run("pacdiff", &["-o"], Duration::from_secs(60))
        .map(|o| o.stdout.lines().filter(|l| !l.trim().is_empty()).count())
        .unwrap_or(0)
}

/// Orphans and the package cache.
pub fn cleanup(cfg: &Config, progress: &Progress) {
    progress.step("cleanup", "Cleaning up after the update", 0);
    progress.tick_start(1, None);
    if cfg.remove_orphans
        && let Some(out) = proc::run("pacman", &["-Qtdq"], Duration::from_secs(60))
    {
        let orphans: Vec<&str> = out.stdout.split_whitespace().collect();
        if !orphans.is_empty() {
            log::info(format!("Removing {} orphaned package(s)", orphans.len()));
            if !log::run_logged(Command::new("pacman").args(["-Rns", "--noconfirm", "--color", "never"]).args(&orphans)) {
                log::warn("Removing orphans failed");
            }
        }
    }
    if cfg.clean_cache && proc::have("paccache") {
        // paccache's dry run says what a real run frees; the real run says
        // little.
        let k = format!("-k{}", cfg.keep_old);
        let mut saved = Vec::new();
        for args in [vec!["-d", "--nocolor", k.as_str()], vec!["-du", "--nocolor", "-k0"]] {
            if let Some(out) = proc::run("paccache", &args, Duration::from_secs(120)) {
                for line in out.stdout.lines() {
                    if let Some(i) = line.find("disk space saved: ") {
                        saved.push(line[i..].trim_end_matches(')').to_string());
                    }
                }
            }
        }
        let saved = if saved.is_empty() { String::new() } else { format!(": {}", saved.join(", ")) };
        log::info(format!("Trimming the package cache (keeping {} old version(s)){saved}", cfg.keep_old));
        if !log::run_logged(Command::new("paccache").args(["-r", "--nocolor", &k])) {
            log::warn("paccache -r failed");
        }
        if !log::run_logged(Command::new("paccache").args(["-ru", "--nocolor", "-k0"])) {
            log::warn("paccache -ru failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_pacman_output() {
        let text = "Packages (3) a-1 b-2 c-3\n\nTotal Download Size:   2.00 MiB\n:: Proceed\n:: Retrieving packages...\n a-1-1-x86_64 downloading...\n b-2-1-any downloading...\nchecking keyring...\n( 1/3) upgrading a                 [###]\n";
        let w = watch(text);
        assert_eq!(w.total, 3);
        assert_eq!(w.size, 2 * 1_048_576);
        assert!(w.fetch && w.commit && !w.hooks);
        assert_eq!(w.started, 2);
        assert_eq!(bare(&w.file), "b");
        assert_eq!((w.counted, w.outof, w.name.as_str()), (1, 3, "a"));
        let w = watch("upgrading glibc...\nupgrading foo...\n:: Running post-transaction hooks...\n");
        assert_eq!((w.n, w.name.as_str(), w.hooks), (2, "foo", true));
    }

    #[test]
    fn sorts_failures_and_finds_blockers() {
        assert_eq!(classify("error: failed to prepare transaction (could not satisfy dependencies)"), Failure::Dependency);
        assert_eq!(classify("foo: /usr/bin/x exists in filesystem"), Failure::Filesystem);
        assert_eq!(classify("error: foo: signature from \"X\" is unknown trust"), Failure::Keyring);
        assert_eq!(classify(":: a and b are in conflict"), Failure::Conflict);
        let log = ":: removing libfoo breaks dependency 'libfoo' required by bar\n:: unable to satisfy dependency 'x' required by baz\n";
        assert_eq!(blockers(log), vec!["baz".to_string(), "libfoo".to_string()]);
    }

    #[test]
    fn knows_when_pacman_installs() {
        assert!(!committed(":: Retrieving packages...\n foo downloading...\n"));
        assert!(committed("checking package integrity...\n"));
        assert!(committed("( 1/2) upgrading foo"));
    }
}
