//! `_progress`: the update's progress entry on the desktop.
//!
//! Started by the update run once per logged-in user, inside that user's
//! session. Reads one order per line on stdin and turns it into the entry
//! Plasma shows while Dolphin copies files:
//!
//!   info<TAB>text              headline, already translated
//!   note<TAB>text              shown instead of the headline, empty clears
//!   detail<TAB>name<TAB>value  a labelled line under "Details"
//!   log<TAB>name<TAB>line...   the run log's last lines, one per field
//!   total<TAB>n / done<TAB>n   the item counter
//!   percent<TAB>n              overall progress, 0-100
//!   suspended<TAB>0|1          paused, or running again
//!   end[<TAB>message]          finish; a message marks the job as failed
//!   killed                     finish after a cancel, without a message
//!
//! It prints "ready" once the entry is open, then a line per click on Cancel
//! or Pause: "cancel", "pause" or "resume".
//!
//! A process of its own, because Plasma withdraws the entry the moment the
//! D-Bus connection that asked for it goes away.

use std::collections::HashMap;
use std::io::Write;
use std::time::Duration;
use zbus::Connection;

const JOB_SERVICE: &str = "org.kde.JobViewServer";
const JOB_PATH: &str = "/JobViewServer";
const JOB_IFACE: &str = "org.kde.JobViewV2";
const DESKTOP_ENTRY: &str = "cachy-auto-update";

/// KJob::Killable | KJob::Suspendable
const CAPABILITIES: i32 = 1 | 2;

/// Plasma reads a description that looks like a link ("Info:", a path) as
/// one. An invisible word joiner in front keeps it plain text.
const NO_LINK: &str = "\u{2060}";

struct Job {
    conn: Connection,
    path: Option<zbus::OwnedObjectPath>,
}

impl Job {
    async fn call<B: serde_like::Body>(&mut self, method: &str, body: &B) {
        let Some(path) = self.path.clone() else { return };
        let r = self.conn.call_method(Some(JOB_SERVICE), path.as_str(), Some(JOB_IFACE), method, body).await;
        if r.is_err() {
            // The desktop went away (a logout, a plasmashell restart), or
            // dropped the entry after Cancel. The update goes on without it.
            self.path = None;
        }
    }

    /// Always ends explicitly: a job whose owner just disappears is shown as
    /// a crash. The error code decides the label. 100 is a user-defined
    /// error, shown with the message. 1 is KilledJobError, which Plasma
    /// drops without a word: right for a cancelled run.
    async fn close(&mut self, message: &str, code: Option<u32>) {
        let code = code.or((!message.is_empty()).then_some(100));
        if let Some(code) = code {
            self.call("setError", &(code,)).await;
        }
        self.call("terminate", &(message,)).await;
        self.path = None;
    }
}

/// zbus's bound for a message body, named once.
mod serde_like {
    pub trait Body: serde::Serialize + zbus::zvariant::DynamicType {}
    impl<T: serde::Serialize + zbus::zvariant::DynamicType> Body for T {}
}

/// What the entry shows. While paused, progress is held back and shown on
/// resume, so a bar that is not moving does not creep along.
struct Entry {
    label: String,
    note: String,
    paused: bool,
    held: Vec<(&'static str, u64)>,
    percent: u32,
    target: u32,
}

pub fn main(args: &[String]) -> i32 {
    let controls = args.iter().any(|a| a == "--controls");
    let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(_) => return 0,
    };
    rt.block_on(run(controls));
    0
}

async fn open(controls: bool) -> Option<Job> {
    let conn = Connection::session().await.ok()?;
    let hints: HashMap<&str, zbus::Variant> = HashMap::new();
    let caps = if controls { CAPABILITIES } else { 0 };
    let reply = conn
        .call_method(
            Some(JOB_SERVICE),
            JOB_PATH,
            Some("org.kde.JobViewServerV2"),
            "requestView",
            &(DESKTOP_ENTRY, caps, hints),
        )
        .await
        .ok()?;
    let path: zbus::OwnedObjectPath = reply.body().deserialize().ok()?;
    Some(Job { conn, path: Some(path) })
}

async fn run(controls: bool) {
    use tokio::io::AsyncBufReadExt;
    // Lines, not read_line: its next_line is safe to drop inside select!.
    let mut stdin = tokio::io::BufReader::new(tokio::io::stdin()).lines();
    let Some(mut job) = tokio::time::timeout(Duration::from_secs(10), open(controls)).await.ok().flatten() else {
        // No job server on this desktop (anything that is not Plasma). The
        // run sees no "ready" and stops writing.
        return;
    };
    let mut clicks = match (controls, job.path.clone()) {
        (true, Some(path)) => {
            let rule =
                zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal).interface(JOB_IFACE).path(path).build();
            match rule {
                Ok(rule) => zbus::MessageStream::for_match_rule(rule, &job.conn, std::num::NonZeroUsize::new(16)).await.ok(),
                Err(_) => None,
            }
        }
        _ => None,
    };
    let say = |s: &str| {
        let mut out = std::io::stdout();
        let _ = writeln!(out, "{s}");
        let _ = out.flush();
    };
    say("ready");

    let mut e = Entry { label: String::new(), note: String::new(), paused: false, held: Vec::new(), percent: 0, target: 0 };
    let mut slide = tokio::time::interval(Duration::from_millis(100));
    let mut clicks_done = false;
    loop {
        tokio::select! {
            line = stdin.next_line() => {
                let Ok(Some(line)) = line else { break };
                let fields: Vec<&str> = line.split('\t').collect();
                if !handle(&mut job, &mut e, &fields).await {
                    return;
                }
            }
            msg = next_click(&mut clicks), if !clicks_done => match msg.as_deref() {
                Some("cancelRequested") => say("cancel"),
                Some("suspendRequested") => say("pause"),
                Some("resumeRequested") => say("resume"),
                Some(_) => {}
                // The stream ended: no more clicks, and no spinning on it.
                None => clicks_done = true,
            },
            _ = slide.tick(), if !e.paused && e.percent < e.target => {
                // A quarter of the way each time, so a jump in the numbers is a
                // short slide on the bar.
                e.percent += ((e.target - e.percent) / 4).max(1);
                job.call("setPercent", &(e.percent,)).await;
            }
        }
    }
    // The run went away without saying goodbye.
    job.close("", None).await;
}

/// The next signal's name. Other messages on the stream give an empty name.
async fn next_click(stream: &mut Option<zbus::MessageStream>) -> Option<String> {
    use futures_util::StreamExt;
    match stream {
        Some(s) => match s.next().await? {
            Ok(msg) => Some(msg.header().member().map(|m| m.to_string()).unwrap_or_default()),
            Err(_) => Some(String::new()),
        },
        None => std::future::pending().await,
    }
}

/// Applies one order. False once the entry is finished.
async fn handle(job: &mut Job, e: &mut Entry, f: &[&str]) -> bool {
    let arg = f.get(1).copied().unwrap_or("");
    let num = || arg.trim().parse::<u64>().unwrap_or(0);
    match f[0] {
        "info" => {
            e.label = arg.to_string();
            show(job, e).await;
        }
        "note" => {
            e.note = arg.to_string();
            show(job, e).await;
        }
        "detail" if f.len() > 2 => {
            job.call("setDescriptionField", &(0u32, arg, format!("{NO_LINK}{}", f[2]))).await;
        }
        // Field 1, and there is no field 2: the job model carries two.
        "log" if f.len() > 2 => {
            job.call("setDescriptionField", &(1u32, arg, format!("{NO_LINK}{}", f[2..].join("\n")))).await;
        }
        "total" => progress(job, e, "setTotalAmount", num()).await,
        "done" => progress(job, e, "setProcessedAmount", num()).await,
        "percent" => e.target = e.target.max((num() as u32).min(100)),
        "suspended" => {
            e.paused = arg == "1";
            // Plasma marks a paused entry itself, so the note has done its job.
            e.note.clear();
            show(job, e).await;
            job.call("setSuspended", &(e.paused,)).await;
            if !e.paused {
                for (method, n) in std::mem::take(&mut e.held) {
                    job.call(method, &(n, "items")).await;
                }
            }
        }
        "end" => {
            // No time left to slide: the entry is about to go.
            if e.target > e.percent {
                e.percent = e.target;
                job.call("setPercent", &(e.percent,)).await;
            }
            job.close(arg, None).await;
            return false;
        }
        "killed" => {
            job.close("", Some(1)).await;
            return false;
        }
        _ => {}
    }
    true
}

async fn show(job: &mut Job, e: &Entry) {
    let text = if e.note.is_empty() { e.label.clone() } else { e.note.clone() };
    job.call("setInfoMessage", &(text,)).await;
}

async fn progress(job: &mut Job, e: &mut Entry, method: &'static str, n: u64) {
    if e.paused {
        e.held.retain(|(m, _)| *m != method);
        e.held.push((method, n));
    } else {
        job.call(method, &(n, "items")).await;
    }
}
