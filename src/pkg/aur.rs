//! AUR packages.
//!
//! makepkg (and so paru and yay) refuse to run as root, so this part runs as
//! the locked "cachy-auto-update" system account instead. sysusers.d makes it
//! with no password and no shell. It may run /usr/bin/pacman without a
//! password (/etc/sudoers.d/cachy-auto-update), which is what lets the
//! helper install what it built with nobody there.

use crate::config::Config;
use crate::control::{Control, Kind};
use crate::paths::{BUILD_USER, paths};
use crate::progress::Progress;
use crate::{log, proc, state, users};
use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Seek, SeekFrom};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A held package is tried again once the AUR has a new version, or after
/// this long, so a build that failed only because the network dropped does
/// not stay held for months.
const RETRY_AFTER: u64 = 3 * 86400;

/// About how long one AUR build takes, for a package never built here.
/// Measured on real runs, where most packages are a download packed again.
const BUILD_SECONDS: u64 = 45;

/// Where AUR helpers build, below a cache folder: a folder per package, each
/// with a PKGBUILD.
const CACHES: &[&str] = &["paru/clone", "yay", "Shelly"];

/// Programs that build in those folders. While one runs, nothing is cleaned.
const BUILDERS: &[&str] = &["paru", "yay", "makepkg", "shelly", "shelly-ui"];

/// What the AUR step did. It never fails the run: a package that does not
/// build is held back and the others go on.
#[derive(Default)]
pub struct Outcome {
    pub count: u64,
    pub held: Vec<String>,
}

/// The helper to use, as AURHelper says.
fn detect(cfg: &Config, quiet: bool) -> Option<&'static str> {
    if cfg.aur_helper != "auto" {
        let h = crate::config::AUR_HELPERS.iter().find(|h| **h == cfg.aur_helper)?;
        if proc::have(h) {
            return Some(h);
        }
        if !quiet {
            log::warn(format!("Configured AUR helper '{h}' not found"));
        }
        return None;
    }
    ["paru", "yay", "pikaur"].into_iter().find(|h| proc::have(h))
}

/// Whether an AUR helper exists at all, without a word in the log.
pub fn helper_installed(cfg: &Config) -> bool {
    detect(cfg, true).is_some()
}

/// A small environment: the helper gets its own HOME and cache, so nothing
/// it downloads lands in a person's home.
fn as_build_user(program: &str) -> Command {
    let p = paths();
    let home = p.build_home();
    let mut c = Command::new("runuser");
    c.args(["-u", BUILD_USER, "--", "env"])
        .arg(format!("HOME={}", home.display()))
        .arg(format!("USER={BUILD_USER}"))
        .arg(format!("LOGNAME={BUILD_USER}"))
        .arg(format!("XDG_CACHE_HOME={}", p.cache.display()))
        .arg(format!("XDG_CONFIG_HOME={}/.config", home.display()))
        .arg(format!("XDG_DATA_HOME={}/.local/share", home.display()))
        .arg("PATH=/usr/local/sbin:/usr/local/bin:/usr/bin")
        .arg("LC_ALL=C")
        .arg(program);
    c
}

/// The helper, when everything the AUR step needs is in place.
fn ready(cfg: &Config, quiet: bool) -> Option<&'static str> {
    let say = |level: fn(String), msg: &str| {
        if !quiet {
            level(msg.to_string());
        }
    };
    let Some(helper) = detect(cfg, quiet) else {
        say(log::info, "No AUR helper installed; skipping AUR updates");
        return None;
    };
    if !proc::quiet(Command::new("getent").args(["passwd", BUILD_USER])) {
        say(log::warn, &format!("Build account '{BUILD_USER}' is missing; skipping AUR updates"));
        return None;
    }
    // -l asks sudo whether the command is allowed; -n means it never waits for
    // a password. `sudo -n true` would fail by design: the rule is for pacman
    // alone.
    let mut sudo = as_build_user("sudo");
    sudo.args(["-n", "-l", "/usr/bin/pacman"]);
    if !proc::quiet(&mut sudo) {
        say(
            log::warn,
            "Build account cannot run pacman without a password; check /etc/sudoers.d/cachy-auto-update",
        );
        return None;
    }
    if !proc::have("makepkg") {
        say(log::warn, "makepkg not found (base-devel missing); skipping AUR updates");
        return None;
    }
    Some(helper)
}

/// The flags that make a helper silent.
fn helper_args(helper: &str, cfg: &Config) -> Vec<&'static str> {
    let mut a = match helper {
        "paru" => vec!["-Sua", "--noconfirm", "--skipreview", "--removemake", "--cleanafter", "--color", "never"],
        "yay" => vec![
            "-Sua",
            "--noconfirm",
            "--removemake",
            "--cleanafter",
            "--color",
            "never",
            "--answerclean",
            "All",
            "--answerdiff",
            "None",
            "--answeredit",
            "None",
            "--answerupgrade",
            "None",
        ],
        _ => vec!["-Sua", "--noconfirm", "--noedit"],
    };
    if cfg.devel && helper != "pikaur" {
        a.push("--devel");
    }
    a
}

/// Pending AUR updates as name and new version. Packages pacman.conf
/// ignores are left out: the helper skips them anyway.
fn list(helper: &str) -> Vec<(String, String)> {
    let mut cmd = as_build_user(helper);
    cmd.arg("-Qua");
    let Some(out) = proc::output(&mut cmd, Duration::from_secs(600)) else { return Vec::new() };
    out.stdout
        .lines()
        .filter(|l| !l.contains("[ignored]"))
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            (f.len() >= 2 && crate::config::is_package_name(f[0])).then(|| (f[0].to_string(), f[f.len() - 1].to_string()))
        })
        .collect()
}

/// How long each package should take, for the bar.
#[derive(Clone, Default)]
struct Estimate {
    /// package -> the PKGBUILD it comes from
    base: HashMap<String, String>,
    /// package -> seconds
    est: HashMap<String, u64>,
    work: u64,
}

/// A package should take as long as it did before (state file aur_times).
/// One new here gets BUILD_SECONDS, shared with the other packages from the
/// same PKGBUILD: those are built in one go, so they are one piece of work.
fn estimate(names: &[String]) -> Estimate {
    let mut e = Estimate::default();
    for n in names {
        e.base.insert(n.clone(), n.clone());
    }
    if let Some(out) = proc::output(Command::new("pacman").arg("-Q").args(names), Duration::from_secs(60)) {
        for line in out.stdout.lines() {
            let mut it = line.split_whitespace();
            let (Some(name), Some(ver)) = (it.next(), it.next()) else { continue };
            if !e.base.contains_key(name) {
                continue;
            }
            let desc = std::fs::read_to_string(format!("/var/lib/pacman/local/{name}-{ver}/desc")).unwrap_or_default();
            let mut lines = desc.lines();
            while let Some(l) = lines.next() {
                if l == "%BASE%" {
                    if let Some(b) = lines.next().filter(|b| !b.is_empty()) {
                        e.base.insert(name.to_string(), b.to_string());
                    }
                    break;
                }
            }
        }
    }
    let known = times();
    let mut parts: HashMap<&str, u64> = HashMap::new();
    for n in names {
        *parts.entry(e.base[n].as_str()).or_default() += 1;
    }
    for n in names {
        let took = known.get(n).copied().unwrap_or(BUILD_SECONDS / parts[e.base[n].as_str()]).max(1);
        e.est.insert(n.clone(), took);
        e.work += took;
    }
    e
}

fn times() -> BTreeMap<String, u64> {
    state::read_or("aur_times", "")
        .lines()
        .filter_map(|l| {
            let (n, t) = l.split_once(' ')?;
            let t: u64 = t.trim().parse().ok()?;
            (t > 0).then(|| (n.to_string(), t))
        })
        .collect()
}

/// When each package was installed, in seconds since the builds began.
type Marks = Arc<Mutex<Vec<(String, u64)>>>;

/// How far the step is, for the bar's ticker: how many packages are
/// installed, how many seconds of work that was, and how long the build in
/// progress should take. Also notes when each package got installed.
///
/// The helper passes pacman's "upgrading foo..." on to the run log, and
/// makepkg says which PKGBUILD it works on.
fn built(e: &Estimate, offset: u64, marks: &Marks, began: Instant) -> (u64, u64, u64) {
    let mut text = String::new();
    if let Ok(mut f) = std::fs::File::open(paths().runlog()) {
        let _ = f.seek(SeekFrom::Start(offset));
        let mut buf = Vec::new();
        let _ = f.read_to_end(&mut buf);
        text = String::from_utf8_lossy(&buf).into_owned();
    }
    let mut now = String::new();
    let mut done: Vec<&str> = Vec::new();
    let mut m = marks.lock().unwrap();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("==> Making package: ") {
            now = rest.split_whitespace().next().unwrap_or("").to_string();
            continue;
        }
        let Some(rest) = line.strip_prefix("upgrading ").or_else(|| line.strip_prefix("installing ")) else { continue };
        let Some(name) = rest.strip_suffix("...") else { continue };
        if name.contains(' ') || !e.est.contains_key(name) {
            continue;
        }
        if !done.contains(&name) {
            done.push(name);
        }
        if !m.iter().any(|(n, _)| n == name) {
            m.push((name.to_string(), began.elapsed().as_secs()));
        }
    }
    let (mut secs, mut piece) = (0, 0);
    for (name, est) in &e.est {
        if done.contains(&name.as_str()) {
            secs += est;
        } else if e.base[name] == now {
            piece += est;
        }
    }
    // Between two builds makepkg has not named the next one yet. Then it is
    // taken to be an average one.
    let left = e.est.len().saturating_sub(done.len()) as u64;
    if piece == 0 && left > 0 {
        piece = e.work.saturating_sub(secs) / left;
    }
    (done.len() as u64, secs, piece.max(1))
}

/// Remembers how long each package took, for the next run's bar. Packages
/// installed in the same second came from one build and share its time.
/// Half old, half new, so one slow day does not count too much.
fn learn(marks: &Marks) {
    let mut t = times();
    let m = marks.lock().unwrap().clone();
    let (mut i, mut prev) = (0, 0u64);
    while i < m.len() {
        let mut j = i + 1;
        while j < m.len() && m[j].1 == m[i].1 {
            j += 1;
        }
        let took = (m[i].1.saturating_sub(prev) / (j - i) as u64).max(1);
        prev = m[i].1;
        for (name, _) in &m[i..j] {
            let v = t.get(name).map(|old| (old + took) / 2).unwrap_or(took);
            t.insert(name.clone(), v);
        }
        i = j;
    }
    if t.is_empty() {
        return;
    }
    let text: Vec<String> = t.iter().map(|(n, s)| format!("{n} {s}")).collect();
    let _ = state::write("aur_times", &text.join("\n"));
}

/// How many AUR updates are pending, and the helper that would build them.
pub fn pending(cfg: &Config) -> Option<(usize, &'static str)> {
    let helper = ready(cfg, false)?;
    Some((list(helper).len(), helper))
}

/// How many seconds of builds are pending. Quiet; only for the bar.
pub fn forecast(cfg: &Config) -> Option<u64> {
    let helper = ready(cfg, true)?;
    let names: Vec<String> = list(helper).into_iter().map(|(n, _)| n).collect();
    if names.is_empty() {
        return Some(0);
    }
    Some(estimate(&names).work)
}

/// One run of the helper. After a pause it runs again: packages installed
/// before the pause are not pending any more.
fn build(helper: &str, args: &[String], progress: &Progress, control: &Control) -> bool {
    loop {
        let mut cmd = as_build_user(helper);
        cmd.args(args);
        if control.run(progress, Kind::Aur, None, &mut cmd, None).is_some_and(|s| s.success()) {
            return true;
        }
        if control.stopped() && control.wait(progress) {
            continue;
        }
        return false;
    }
}

pub fn update(cfg: &Config, progress: &Progress, control: &Control) -> Outcome {
    let mut out = Outcome::default();
    // Left over from before packages were held back.
    state::clear("aur_failures");

    // No helper, no base-devel: a step that cannot run holds no share of the bar.
    let Some(helper) = ready(cfg, false) else {
        progress.drop_steps(&["aur"]);
        return out;
    };
    progress.forecast();
    progress.step("aur", "Updating AUR packages", 0);
    progress.tick_start(1, None);
    let want: BTreeMap<String, String> = list(helper).into_iter().collect();
    progress.tick_stop();
    if want.is_empty() {
        log::info("No AUR updates pending");
        state::clear("aur_held");
        progress.drop_steps(&["aur"]);
        return out;
    }

    // Still held: the same version is pending and it is not time to try again.
    let now = state::now();
    let mut held: BTreeMap<String, (String, u64)> = BTreeMap::new();
    for line in state::read_or("aur_held", "").lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        let [name, ver, since] = f[..] else { continue };
        let Ok(since) = since.parse::<u64>() else { continue };
        if want.get(name).is_some_and(|v| v == ver) && now.saturating_sub(since) < RETRY_AFTER {
            held.insert(name.to_string(), (ver.to_string(), since));
        }
    }
    let todo: Vec<String> = want.keys().filter(|n| !held.contains_key(*n)).cloned().collect();

    if todo.is_empty() {
        progress.drop_steps(&["aur"]);
    } else {
        log::info(format!("Updating {} AUR package(s) with {helper}", todo.len()));
        // Each package gets the part of the step it should take. The bar
        // counts the ones pacman installed; in between it goes by the clock.
        let est = estimate(&todo);
        let marks: Marks = Arc::default();
        let offset = std::fs::metadata(paths().runlog()).map(|m| m.len()).unwrap_or(0);
        let began = Instant::now();
        progress.weigh("aur", est.work);
        progress.step("aur", "Updating AUR packages", todo.len() as u64);
        {
            let (e, m) = (est.clone(), marks.clone());
            progress.tick_start(
                todo.len() as u64,
                Some(Box::new(move || {
                    let (n, secs, piece) = built(&e, offset, &m, began);
                    (n, Some((secs, piece)))
                })),
            );
        }
        let args: Vec<String> = helper_args(helper, cfg).into_iter().map(String::from).collect();
        let join = |names: Vec<&String>| names.into_iter().cloned().collect::<Vec<_>>().join(",");
        let mut first = args.clone();
        if !held.is_empty() {
            first.push("--ignore".into());
            first.push(join(held.keys().collect()));
        }
        let mut clean = false;
        // All at once first. If that fails, each package still pending gets a
        // try on its own, so only the broken one is held back.
        if build(helper, &first, progress, control) {
            out.count = todo.len() as u64;
            clean = true;
        } else if control.stopped() || control.cancelled() {
            log::info("AUR update was stopped from the desktop");
        } else {
            let left: Vec<String> =
                list(helper).into_iter().map(|(n, _)| n).filter(|n| want.contains_key(n) && !held.contains_key(n)).collect();
            out.count = (todo.len() - left.len().min(todo.len())) as u64;
            for name in &left {
                if left.len() > 1 {
                    let mut skip: Vec<&String> = held.keys().collect();
                    skip.extend(left.iter().filter(|o| *o != name && !held.contains_key(*o)));
                    log::info(format!("Trying {name} on its own"));
                    let mut a = args.clone();
                    a.push("--ignore".into());
                    a.push(join(skip));
                    if build(helper, &a, progress, control) {
                        out.count += 1;
                        continue;
                    }
                }
                if control.cancelled() {
                    break;
                }
                log::warn(format!("Could not build {name}; holding it back"));
                held.insert(name.clone(), (want[name].clone(), now));
            }
        }
        progress.tick_stop();
        // Only a run that went through in one go says how long a build takes.
        if clean {
            built(&est, offset, &marks, began);
            learn(&marks);
            progress.item(todo.len() as u64, todo.len() as u64);
        }
    }

    if held.is_empty() {
        state::clear("aur_held");
        return out;
    }
    let lines: Vec<String> = held.iter().map(|(n, (v, s))| format!("{n} {v} {s}")).collect();
    let _ = state::write("aur_held", &lines.join("\n"));
    out.held = held.keys().cloned().collect();
    log::info(format!("Holding back AUR package(s): {}", out.held.join(" ")));
    out
}

/// Removes build leftovers from the AUR helpers' caches: built packages,
/// downloaded sources, and the src/ and pkg/ build folders. PKGBUILDs,
/// patches and git data stay. Without it ~/.cache/paru keeps every version
/// of every -bin package: 32 GB on one laptop.
pub fn clean_caches(cfg: &Config) {
    if !cfg.clean_aur_cache {
        return;
    }
    let procs = proc::processes();
    if let Some(p) = BUILDERS.iter().find(|p| proc::running(&procs, p)) {
        log::info(format!("Not cleaning the AUR caches while {p} is running"));
        return;
    }
    let mut places = vec![(BUILD_USER.to_string(), paths().cache.clone())];
    for u in users::human_users() {
        places.push((u.name.clone(), std::path::PathBuf::from(&u.home).join(".cache")));
    }
    let mut total = 0u64;
    for (user, base) in places {
        if !base.is_dir() {
            continue;
        }
        for c in CACHES {
            let dir = base.join(c);
            if dir.is_dir() {
                total += clean_cache(&user, &dir);
            }
        }
    }
    if total >= 1_048_576 {
        log::info(format!("Cleaned the AUR caches: {} MiB freed", total / 1_048_576));
    }
}

/// How many bytes it freed. Runs as the owner of the cache, so a symlink in
/// there can never make root delete something else. Only what a build made
/// goes: built packages, anything over 1 MiB (the AUR takes no file that
/// large, so it was downloaded) and the build folders.
fn clean_cache(user: &str, dir: &std::path::Path) -> u64 {
    const SCRIPT: &str = r#"
        before=$(du -sb "$1" 2>/dev/null | cut -f1)
        for d in "$1"/*/; do
            [ -L "${d%/}" ] && continue
            [ -f "${d}PKGBUILD" ] || continue
            rm -rf -- "${d}src" "${d}pkg"
            find "$d" -mindepth 1 -maxdepth 1 -type f \( -name "*.pkg.tar*" -o -size +1M \) -delete
        done
        after=$(du -sb "$1" 2>/dev/null | cut -f1)
        echo $(( ${before:-0} - ${after:-0} ))
    "#;
    let mut cmd = Command::new("runuser");
    cmd.args(["-u", user, "--", "sh", "-c", SCRIPT, "sh"]).arg(dir);
    proc::output(&mut cmd, Duration::from_secs(600)).and_then(|o| o.stdout.trim().parse().ok()).unwrap_or(0)
}
