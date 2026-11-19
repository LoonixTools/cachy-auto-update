//! The people on this machine, and running things on their behalf.
//!
//! The update runs as a root service, but some of the work (per-user
//! Flatpaks, AppImages, notifications, the progress bar) only makes sense
//! inside a user's own session.

use crate::proc;
use std::process::Command;
use std::time::Duration;

/// Lowest and highest uid of a human account, as /etc/login.defs on Arch.
const UID_MIN: u32 = 1000;
const UID_MAX: u32 = 60000;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct User {
    pub name: String,
    pub uid: u32,
    pub home: String,
}

fn passwd() -> String {
    proc::run("getent", &["passwd"], Duration::from_secs(10))
        .map(|o| o.stdout)
        .unwrap_or_else(|| std::fs::read_to_string("/etc/passwd").unwrap_or_default())
}

fn parse_passwd(line: &str) -> Option<(User, String)> {
    let f: Vec<&str> = line.split(':').collect();
    if f.len() < 7 {
        return None;
    }
    let uid = f[2].parse().ok()?;
    Some((User { name: f[0].to_string(), uid, home: f[5].to_string() }, f[6].to_string()))
}

/// Every account that is not a system account and has a real shell.
pub fn human_users() -> Vec<User> {
    passwd()
        .lines()
        .filter_map(parse_passwd)
        .filter(|(u, shell)| {
            (UID_MIN..=UID_MAX).contains(&u.uid)
                && !shell.is_empty()
                && !shell.ends_with("/nologin")
                && !shell.ends_with("/false")
        })
        .map(|(u, _)| u)
        .collect()
}

pub fn home_of(name: &str) -> Option<String> {
    passwd().lines().filter_map(parse_passwd).find(|(u, _)| u.name == name).map(|(u, _)| u.home)
}

/// Everyone with an active graphical session, once each: the people a
/// notification can reach.
pub fn session_users() -> Vec<User> {
    let mut out: Vec<User> = Vec::new();
    if !proc::have("loginctl") {
        return out;
    }
    let Some(list) = proc::run("loginctl", &["list-sessions", "--no-legend"], Duration::from_secs(10)) else {
        return out;
    };
    for id in list.stdout.lines().filter_map(|l| l.split_whitespace().next()) {
        let Some(props) = proc::run(
            "loginctl",
            &["show-session", id, "--property=Name", "--property=User", "--property=Class", "--property=Type", "--property=State"],
            Duration::from_secs(10),
        ) else {
            continue;
        };
        let get = |k: &str| {
            props.stdout.lines().find_map(|l| l.strip_prefix(k).and_then(|r| r.strip_prefix('='))).unwrap_or("").to_string()
        };
        let (name, uid) = (get("Name"), get("User").parse::<u32>().ok());
        if get("Class") != "user" || !matches!(get("State").as_str(), "active" | "online") {
            continue;
        }
        if !matches!(get("Type").as_str(), "wayland" | "x11" | "mir") {
            continue;
        }
        let (Some(uid), false) = (uid, name.is_empty()) else { continue };
        if out.iter().any(|u| u.uid == uid) {
            continue;
        }
        let home = home_of(&name).unwrap_or_else(|| format!("/home/{name}"));
        out.push(User { name, uid, home });
    }
    out.sort();
    out
}

/// A command run as that user, with an environment shaped like their
/// session. LC_ALL goes: the service sets it to C for parseable output, but
/// anything a person reads follows their own locale.
pub fn as_user(user: &User, program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut cmd = Command::new("runuser");
    cmd.args(["-u", &user.name, "--", "env", "--unset=LC_ALL"]);
    cmd.arg(format!("HOME={}", user.home));
    cmd.arg(format!("USER={}", user.name));
    cmd.arg(format!("LOGNAME={}", user.name));
    cmd.arg(format!("XDG_RUNTIME_DIR=/run/user/{}", user.uid));
    let bus = format!("/run/user/{}/bus", user.uid);
    if std::path::Path::new(&bus).exists() {
        cmd.arg(format!("DBUS_SESSION_BUS_ADDRESS=unix:path={bus}"));
    }
    cmd.arg(program);
    cmd
}

/// Whether the user can run a program (it is on their PATH).
pub fn user_has(user: &User, program: &str) -> bool {
    proc::quiet(as_user(user, "sh").args(["-c", "command -v \"$1\" >/dev/null 2>&1", "sh", program]))
}

/// Best guess at the locale the user's desktop runs in, so a notification
/// does not arrive in English on a German machine.
pub fn locale_of(user: &User) -> String {
    if std::path::Path::new(&format!("/run/user/{}", user.uid)).is_dir()
        && let Some(out) = proc::output(as_user(user, "systemctl").args(["--user", "show-environment"]), Duration::from_secs(10))
        && let Some(lang) = out.stdout.lines().find_map(|l| l.strip_prefix("LANG="))
        && !lang.is_empty()
    {
        return lang.to_string();
    }
    crate::i18n::system_locale().unwrap_or_else(|| "C".into())
}
