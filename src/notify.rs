//! Desktop notifications from a root service.
//!
//! Two ways out:
//!   * live: somebody has a graphical session, so this program runs again as
//!     that user (`_notify`) and talks to their notification server;
//!   * queued: nobody is logged in, so the message waits in a file that the
//!     autostart entry delivers at the next login.
//!
//! Messages travel as a msgid plus arguments rather than as finished text, so
//! a message queued at 04:00 still arrives in the locale of the desktop that
//! shows it.

use crate::config::Config;
use crate::i18n::{tr_in, trf_in, ui_locale};
use crate::paths::{PRETTY, paths, self_exe};
use crate::{proc, state, users};
use std::collections::HashMap;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

const ICON: &str = "cachy-auto-update";
const QUEUE_MAX: usize = 20;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Urgency {
    Low,
    Normal,
    Critical,
}

impl Urgency {
    fn name(self) -> &'static str {
        match self {
            Urgency::Low => "low",
            Urgency::Normal => "normal",
            Urgency::Critical => "critical",
        }
    }
    fn parse(s: &str) -> Self {
        match s {
            "low" => Urgency::Low,
            "critical" => Urgency::Critical,
            _ => Urgency::Normal,
        }
    }
}

/// One message.
pub struct Note<'a> {
    pub urgency: Urgency,
    /// Stays on screen until dismissed. The line drawn: does the machine
    /// still need a person? A finished update does not; a failure does.
    pub linger: bool,
    pub title: &'a str,
    pub body: &'a str,
    pub args: Vec<String>,
}

/// Sends a message that waits in the queue when nobody can see it.
pub fn notify(cfg: &Config, note: Note) {
    notify_tagged(cfg, None, true, note);
}

/// `tag` means at most one message of this kind on screen: the last one with
/// the same tag is withdrawn first, so "Installing updates" gives way to
/// "System updated". Withdraw, then post, rather than replacing: Plasma drops
/// a replace whose old message has already timed out.
///
/// `queue` is false for messages that only matter while somebody looks.
pub fn notify_tagged(cfg: &Config, tag: Option<&str>, queue: bool, note: Note) {
    if !cfg.notifications {
        return;
    }
    let mut delivered = false;
    for user in users::session_users() {
        let locale = users::locale_of(&user);
        let title = tr_in(&locale, note.title);
        let args: Vec<&dyn std::fmt::Display> = note.args.iter().map(|a| a as &dyn std::fmt::Display).collect();
        let body = trf_in(&locale, note.body, &args);
        let key = tag.map(|t| format!("notify_id_{t}_{}", user.name));
        if let Some(key) = &key {
            let prev = state::number(key);
            if prev > 0 {
                let _ = proc::output(
                    users::as_user(&user, self_exe()).args(["_notify-close", &prev.to_string()]),
                    Duration::from_secs(10),
                );
            }
            state::clear(key);
        }
        let mut cmd = users::as_user(&user, self_exe());
        cmd.args(["_notify", note.urgency.name(), if note.linger { "yes" } else { "no" }, title, &body]);
        if let Some(out) = proc::output(&mut cmd, Duration::from_secs(20))
            && out.ok()
        {
            delivered = true;
            if let (Some(key), Ok(id)) = (&key, out.stdout.trim().parse::<u32>()) {
                let _ = state::write(key, &id.to_string());
            }
        }
    }
    if !delivered && queue {
        enqueue(&note);
    }
}

/// Tab-separated records, oldest first. Readable for everyone: delivery runs
/// as the user who logs in. The key is in nanoseconds, so two records never
/// share one.
fn enqueue(note: &Note) {
    let path = paths().notify_queue();
    let _ = std::fs::create_dir_all(&paths().state);
    let ns = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let mut record = format!(
        "{ns}\t{}\t{}\t{}\t{}",
        note.urgency.name(),
        if note.linger { "yes" } else { "no" },
        note.title,
        note.body
    );
    for a in &note.args {
        record.push('\t');
        record.push_str(&a.replace(['\t', '\n'], " "));
    }
    let mut lines: Vec<String> =
        std::fs::read_to_string(&path).unwrap_or_default().lines().map(String::from).collect();
    lines.push(record);
    // Nobody wants three weeks of backlog at login.
    let skip = lines.len().saturating_sub(QUEUE_MAX);
    let text: String = lines[skip..].iter().map(|l| format!("{l}\n")).collect();
    let _ = crate::config::write_atomic(&path, text.as_bytes());
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644));
}

/// Shows the messages that came while nobody was logged in. Runs as the user
/// at login; what they have seen is kept in their own home.
pub fn deliver_queue() {
    let Ok(text) = std::fs::read_to_string(paths().notify_queue()) else { return };
    let state_home = std::env::var_os("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/state")));
    let Some(seen_file) = state_home.map(|d| d.join("cachy-auto-update/notify-seen")) else { return };
    let _ = std::fs::create_dir_all(seen_file.parent().unwrap());
    let seen: u128 = std::fs::read_to_string(&seen_file).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
    let locale = ui_locale();
    let mut newest = seen;
    for line in text.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        let Some(ts) = f.first().and_then(|t| t.parse::<u128>().ok()) else { continue };
        if ts <= seen || f.len() < 5 {
            continue;
        }
        // Records from before the linger field have the title where it sits.
        let (linger, title, body, args) = if f[2] == "yes" || f[2] == "no" {
            (f[2] == "yes", f[3], f[4], &f[5..])
        } else {
            (false, f[2], f[3], &f[4..])
        };
        let args: Vec<&dyn std::fmt::Display> = args.iter().map(|a| a as &dyn std::fmt::Display).collect();
        desktop_notify(Urgency::parse(f[1]), linger, tr_in(&locale, title), &trf_in(&locale, body, &args));
        newest = newest.max(ts);
    }
    let _ = std::fs::write(&seen_file, format!("{newest}\n"));
}

fn block_on<F: std::future::Future>(f: F) -> Option<F::Output> {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().ok()?;
    rt.block_on(async { tokio::time::timeout(Duration::from_secs(15), f).await.ok() })
}

/// Posts a message on the session bus of whoever runs this. The id, if the
/// server took it. -1 is "the server decides", 0 "until dismissed".
pub fn desktop_notify(urgency: Urgency, linger: bool, title: &str, body: &str) -> Option<u32> {
    block_on(async {
        let conn = zbus::Connection::session().await.ok()?;
        let mut hints: HashMap<&str, zbus::Variant> = HashMap::new();
        hints.insert(
            "urgency",
            zbus::Variant::U8(match urgency {
                Urgency::Low => 0,
                Urgency::Normal => 1,
                Urgency::Critical => 2,
            }),
        );
        let expire: i32 = if linger { 0 } else { -1 };
        let reply = conn
            .call_method(
                Some("org.freedesktop.Notifications"),
                "/org/freedesktop/Notifications",
                Some("org.freedesktop.Notifications"),
                "Notify",
                &(PRETTY, 0u32, ICON, title, body, Vec::<&str>::new(), hints, expire),
            )
            .await
            .ok()?;
        reply.body().deserialize::<u32>().ok()
    })
    .flatten()
}

/// Withdraws a message. One that is already gone is fine.
pub fn desktop_close(id: u32) {
    block_on(async {
        let Ok(conn) = zbus::Connection::session().await else { return };
        let _ = conn
            .call_method(
                Some("org.freedesktop.Notifications"),
                "/org/freedesktop/Notifications",
                Some("org.freedesktop.Notifications"),
                "CloseNotification",
                &(id,),
            )
            .await;
    });
}

/// `_notify <urgency> <linger> <title> <body>`, run as the user.
pub fn cli_notify(args: &[String]) -> i32 {
    let [urgency, linger, title, body] = args else { return 2 };
    match desktop_notify(Urgency::parse(urgency), linger == "yes", title, body) {
        Some(id) => {
            let _ = writeln!(std::io::stdout(), "{id}");
            0
        }
        None => 1,
    }
}
