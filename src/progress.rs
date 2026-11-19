//! The update's progress bar.
//!
//! An update can take twenty minutes. This drives the desktop's job list (the
//! same widget that shows a bar while Dolphin copies files), so how far the
//! run is stays visible, and writes the same to a small file that the window
//! and the tray icon read.
//!
//! Plasma ends a job entry as soon as the D-Bus connection that asked for it
//! goes away. So in each session a helper (`_progress`, see job.rs) holds that
//! connection and takes its orders on stdin, one per line. Everything here is
//! the writing end of those pipes, plus the arithmetic that turns "package 120
//! of 260 in the repository step" into one number for the bar. Every step is
//! weighed by how long it should take for the packages this run really has,
//! so the bar moves at about the same pace from start to end.

use crate::config::Config;
use crate::control::Control;
use crate::i18n::tr_in;
use crate::paths::{paths, self_exe};
use crate::users::{self, User};
use crate::{log, proc};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::process::{Child, ChildStdin, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How the run ended, for the bar.
#[derive(Clone, Copy, PartialEq)]
pub enum Outcome {
    Ok,
    Failed,
    Cancelled,
}

/// The first guesses of how many seconds each step takes. The run puts in
/// better numbers as soon as it knows how many packages there are.
const WEIGHTS: &[(&str, u64)] = &[
    ("resolve", 10),
    ("download", 30),
    ("repo", 15),
    ("hooks", 15),
    ("aur", 30),
    ("flatpak", 10),
    ("userflatpak", 5),
    ("appimage", 5),
    ("cleanup", 3),
];

/// About how long one Flatpak app takes.
pub const FLATPAK_SECONDS: u64 = 30;

/// How many lines of the log the entry carries, and how wide each may be.
/// Five lines fit under a notification popup; together they keep one order
/// well inside PIPE_BUF, so the write is atomic.
const TAIL_LINES: usize = 5;
const TAIL_COLS: usize = 120;

struct Session {
    child: Child,
    stdin: Option<ChildStdin>,
    locale: String,
}

struct Inner {
    sessions: Vec<Session>,
    weights: HashMap<&'static str, u64>,
    /// The steps still to do, the current one first.
    plan: Vec<&'static str>,
    step: Option<&'static str>,
    /// Where the bar is, in hundredths of a percent. A step is measured the
    /// same way: `frac` is how far it has come. The bar was at `anchor` when
    /// the step had come `from`, and the steps still to do share the rest of
    /// the bar from there. So nothing that changes the plan can make the bar
    /// jump: it only changes how fast it moves from here on.
    pos: i64,
    anchor: i64,
    from: i64,
    frac: i64,
    shown: i64,
    total: u64,
    done_items: u64,
    /// For work that goes by the clock: when the current item began.
    t0: Instant,
    clock_done: u64,
    // What the status file says.
    label: String,
    note: String,
    detail: String,
    paused: bool,
}

/// A thread that runs next to the update until stopped.
struct Worker {
    stop: Arc<AtomicBool>,
    handle: std::thread::JoinHandle<()>,
}

impl Worker {
    fn spawn(f: impl FnOnce(Arc<AtomicBool>) + Send + 'static) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let s = stop.clone();
        Worker { stop, handle: std::thread::spawn(move || f(s)) }
    }
    fn stop(self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.handle.join();
    }
}

/// Sleeps up to `d`, waking early when asked to stop. True to go on.
fn nap(stop: &AtomicBool, d: Duration) -> bool {
    let end = Instant::now() + d;
    while Instant::now() < end {
        if stop.load(Ordering::Relaxed) {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100).min(end - Instant::now()));
    }
    !stop.load(Ordering::Relaxed)
}

/// Says how many of a step's items are done, and for items of different
/// sizes also the seconds of work done and of the piece in progress.
pub type Counter = Box<dyn FnMut() -> (u64, Option<(u64, u64)>) + Send>;

/// What the forecast found: AUR seconds, system and user Flatpak counts.
type Forecast = (Option<u64>, Option<(u64, u64)>);

#[derive(Clone)]
pub struct Progress {
    inner: Arc<Mutex<Inner>>,
    ticker: Arc<Mutex<Option<Worker>>>,
    tail: Arc<Mutex<Option<Worker>>>,
    forecast: Arc<Mutex<Option<Forecast>>>,
    forecast_done: Arc<AtomicBool>,
}

impl Default for Progress {
    fn default() -> Self {
        Self::new()
    }
}

impl Progress {
    pub fn new() -> Self {
        Progress {
            inner: Arc::new(Mutex::new(Inner {
                sessions: Vec::new(),
                weights: WEIGHTS.iter().copied().collect(),
                plan: Vec::new(),
                step: None,
                pos: 0,
                anchor: 0,
                from: 0,
                frac: 0,
                shown: -1,
                total: 0,
                done_items: 0,
                t0: Instant::now(),
                clock_done: 0,
                label: String::new(),
                note: String::new(),
                detail: String::new(),
                paused: false,
            })),
            ticker: Arc::default(),
            tail: Arc::default(),
            forecast: Arc::default(),
            forecast_done: Arc::default(),
        }
    }

    /// Opens the bar in every graphical session and records the steps this
    /// run will take, so the bar can be shared out between them. `by_hand`
    /// is a click on Update now: then the bar is the answer to it and shows
    /// whatever NotifyOnStart says.
    pub fn begin(&self, cfg: &Config, by_hand: bool, steps: &[&'static str], control: &Control) {
        {
            let mut i = self.inner.lock().unwrap();
            i.plan = steps.to_vec();
            i.write_status();
        }
        if !by_hand && (!cfg.notifications || !cfg.notify_start) {
            return;
        }
        let mut sessions = Vec::new();
        for user in users::session_users() {
            popups(cfg, by_hand, &user);
            if let Some(s) = open_helper(&user, control) {
                sessions.push(s);
            }
        }
        if !sessions.is_empty() {
            self.inner.lock().unwrap().sessions = sessions;
            control.start(self.clone(), cfg.clone());
        }
    }

    /// Whether a bar is shown anywhere.
    pub fn shown(&self) -> bool {
        !self.inner.lock().unwrap().sessions.is_empty()
    }

    /// Takes steps out of the plan, such as AUR when nothing is pending. The
    /// steps that do have work share the rest of the bar.
    pub fn drop_steps(&self, ids: &[&str]) {
        let mut i = self.inner.lock().unwrap();
        let kept: Vec<_> = i.plan.iter().copied().filter(|s| !ids.contains(s)).collect();
        if kept.len() == i.plan.len() {
            return;
        }
        i.pin();
        i.plan = kept;
        if i.step.is_some_and(|s| ids.contains(&s)) {
            i.step = None;
        }
    }

    /// A better guess at how long a step takes. For a step that already
    /// runs, call `step` again afterwards.
    pub fn weigh(&self, id: &'static str, seconds: u64) {
        let mut i = self.inner.lock().unwrap();
        i.pin();
        i.weights.insert(id, seconds.max(1));
    }

    /// Moves on to a step, or starts the next part of the current one. The
    /// bar stays where it is.
    pub fn step(&self, id: &'static str, label: &str, total: u64) {
        let mut i = self.inner.lock().unwrap();
        // A step dropped for having no work is not a step to move to.
        let Some(at) = i.plan.iter().position(|s| *s == id) else { return };
        i.plan.drain(..at);
        i.step = Some(id);
        i.anchor = i.pos;
        i.from = 0;
        i.frac = 0;
        i.t0 = Instant::now();
        i.clock_done = 0;
        i.total = total;
        i.done_items = 0;
        i.label = label.to_string();
        i.detail.clear();
        i.send_each(|loc| format!("info\t{}", tr_in(loc, label)));
        // Always, also for 0: else a step with no count of its own would show
        // the last step's "260 of 260".
        i.send(&format!("total\t{total}"));
        i.send("done\t0");
        i.move_to(0);
        i.write_status();
    }

    /// The counter under the bar. Does not move the bar.
    pub fn count(&self, processed: u64, total: u64) {
        self.inner.lock().unwrap().count(processed, total);
    }

    /// How far through the current step we are, in any unit.
    pub fn at(&self, num: u64, den: u64) {
        if den == 0 {
            return;
        }
        let num = num.min(den);
        self.inner.lock().unwrap().move_to((num * 10000 / den) as i64);
    }

    /// Both at once, for a step that counts its items.
    pub fn item(&self, processed: u64, total: u64) {
        let mut i = self.inner.lock().unwrap();
        i.count(processed, total);
        if total > 0 {
            let p = processed.min(total);
            i.move_to((p * 10000 / total) as i64);
        }
    }

    /// Moves the bar by the clock. `finished` of `total` is done, and the
    /// piece in progress is `piece` of that total.
    pub fn pace(&self, finished: u64, total: u64, piece: u64) {
        self.inner.lock().unwrap().pace(finished, total, piece);
    }

    /// The current step's weight, in seconds.
    pub fn weight(&self) -> u64 {
        let i = self.inner.lock().unwrap();
        i.step.and_then(|s| i.weights.get(s).copied()).unwrap_or(1)
    }

    /// A labelled line under the entry's "Details", such as the package
    /// being unpacked.
    pub fn detail(&self, label: &str, value: &str) {
        let mut i = self.inner.lock().unwrap();
        i.send_each(|loc| format!("detail\t{}\t{value}", tr_in(loc, label)));
        i.detail = value.to_string();
        i.write_status();
    }

    pub fn suspended(&self, on: bool) {
        let mut i = self.inner.lock().unwrap();
        i.send(&format!("suspended\t{}", u8::from(on)));
        i.paused = on;
        i.note.clear();
        i.write_status();
    }

    /// A line shown instead of the step's label until the run pauses. Empty
    /// clears it.
    pub fn note(&self, msgid: &str) {
        let mut i = self.inner.lock().unwrap();
        i.send_each(|loc| format!("note\t{}", if msgid.is_empty() { "" } else { tr_in(loc, msgid) }));
        i.note = msgid.to_string();
        i.write_status();
    }

    /// The last lines of the run log, under "Details".
    fn log_lines(&self, text: &str) {
        let text = text.replace('\t', " ").replace('\n', "\t");
        self.inner.lock().unwrap().send_each(|loc| format!("log\t{}\t{text}", tr_in(loc, "Log")));
    }

    /// `pace` once a second, next to a step that blocks in one long call.
    /// The counter, if given, says how many of the step's items are done, and
    /// for items that are not all the same size also the seconds of work done
    /// and the seconds the piece in progress should take.
    pub fn tick_start(&self, total: u64, mut counter: Option<Counter>) {
        self.tick_stop();
        let me = self.clone();
        let w = Worker::spawn(move |stop| {
            let mut finished = 0;
            while nap(&stop, Duration::from_secs(1)) {
                let (seen, secs) = counter.as_mut().map(|c| c()).unwrap_or((0, None));
                if seen > finished {
                    finished = seen;
                    me.count(finished, total);
                }
                match secs {
                    Some((done, piece)) => {
                        let w = me.weight();
                        me.pace(done, w, piece);
                    }
                    None => me.pace(finished, total, 1),
                }
            }
        });
        *self.ticker.lock().unwrap() = Some(w);
    }

    pub fn tick_stop(&self) {
        let w = self.ticker.lock().unwrap().take();
        if let Some(w) = w {
            w.stop();
        }
    }

    /// Follows the run log, so "Details" shows what the update is doing in
    /// the stretches that have nothing to count, most of all an AUR package
    /// building for a quarter of an hour.
    pub fn tail_start(&self) {
        self.tail_stop();
        if !self.shown() {
            return;
        }
        let me = self.clone();
        let w = Worker::spawn(move |stop| {
            let mut last = String::new();
            while nap(&stop, Duration::from_secs(2)) {
                let now = tail(&paths().runlog());
                if !now.is_empty() && now != last {
                    me.log_lines(&now);
                    last = now;
                }
            }
        });
        *self.tail.lock().unwrap() = Some(w);
    }

    fn tail_stop(&self) {
        let w = self.tail.lock().unwrap().take();
        if let Some(w) = w {
            w.stop();
        }
    }

    /// Counts the AUR and Flatpak updates in the background while the run
    /// looks at the repositories, so their steps have a fair share of the bar
    /// from the start. Only for the bar: each step still looks for itself.
    pub fn forecast_start(&self, cfg: &Config) {
        let cfg = cfg.clone();
        let slot = self.forecast.clone();
        std::thread::spawn(move || {
            let aur = if cfg.aur { crate::pkg::aur::forecast(&cfg) } else { None };
            let flatpak = if cfg.flatpak && proc::have("flatpak") { Some(crate::pkg::flatpak::forecast()) } else { None };
            *slot.lock().unwrap() = Some((aur, flatpak));
        });
    }

    /// Puts the counts in once they are there. Does not wait for them.
    pub fn forecast(&self) {
        if self.forecast_done.load(Ordering::Relaxed) {
            return;
        }
        let Some((aur, flatpak)) = *self.forecast.lock().unwrap() else { return };
        self.forecast_done.store(true, Ordering::Relaxed);
        // A few seconds on top: each step looks for itself before it starts.
        if let Some(aur) = aur {
            self.weigh("aur", 3 + aur);
        }
        if let Some((system, mine)) = flatpak {
            self.weigh("flatpak", 5 + system * FLATPAK_SECONDS);
            self.weigh("userflatpak", 1 + mine * FLATPAK_SECONDS);
        }
    }

    /// Closes the entry. Must run on every way out, a killed run included:
    /// an entry whose owner just vanishes is reported by the desktop as a
    /// crash. Safe to call twice.
    ///
    ///   Ok         the bar fills and the entry goes away
    ///   Failed     the entry goes away where the bar got to, labelled
    ///              as failed with `msgid` when given
    ///   Cancelled  the entry goes away without a word: the click said it all
    pub fn end(&self, outcome: Outcome, msgid: &str) {
        self.tick_stop();
        self.tail_stop();
        let sessions = {
            let mut i = self.inner.lock().unwrap();
            // The last step goes by the clock and never quite gets there. Only
            // a run that worked fills the bar.
            if outcome == Outcome::Ok {
                i.send("percent\t100");
            }
            for s in &mut i.sessions {
                let line = match outcome {
                    Outcome::Cancelled => "killed".to_string(),
                    _ => format!("end\t{}", if msgid.is_empty() { "" } else { tr_in(&s.locale, msgid) }),
                };
                write_line(s, &line);
                s.stdin = None;
            }
            i.plan.clear();
            i.step = None;
            std::mem::take(&mut i.sessions)
        };
        for mut s in sessions {
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(5) {
                if s.child.try_wait().ok().flatten().is_some() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            let _ = s.child.kill();
            let _ = s.child.wait();
        }
        let _ = std::fs::remove_file(paths().progress());
    }
}

impl Inner {
    /// Pins the bar where it is, before the plan changes.
    fn pin(&mut self) {
        self.anchor = self.pos;
        self.from = self.frac;
    }

    fn send(&mut self, line: &str) {
        for s in &mut self.sessions {
            write_line(s, line);
        }
    }

    /// A line per session, in that session's locale.
    fn send_each(&mut self, line: impl Fn(&str) -> String) {
        for s in &mut self.sessions {
            let l = line(&s.locale);
            write_line(s, &l);
        }
    }

    fn move_to(&mut self, frac: i64) {
        // Never backwards. A pacman started again after a conflict counts from
        // the top, and that is no reason for a bar that goes back.
        let frac = frac.min(10000).max(self.frac);
        self.frac = frac;
        let (mut weight, mut rest) = (0i64, 0i64);
        for s in &self.plan {
            let w = self.weights.get(s).copied().unwrap_or(0) as i64;
            if Some(*s) == self.step { weight = w } else { rest += w }
        }
        // This step's share of what was left of the bar at the anchor, times
        // how much of the step has been done since.
        let mut pos = self.anchor;
        let den = weight * (10000 - self.from) + rest * 10000;
        if den > 0 {
            pos += (10000 - pos) * weight * (frac - self.from) / den;
        }
        if pos > self.pos {
            self.pos = pos;
        }
        let pct = self.pos / 100;
        if pct != self.shown {
            self.shown = pct;
            self.send(&format!("percent\t{pct}"));
            self.write_status();
        }
    }

    fn count(&mut self, processed: u64, total: u64) {
        if total == 0 {
            return;
        }
        let processed = processed.min(total);
        if total != self.total {
            self.total = total;
            self.send(&format!("total\t{total}"));
        }
        self.done_items = processed;
        self.send(&format!("done\t{processed}"));
        self.write_status();
    }

    fn pace(&mut self, finished: u64, total: u64, piece: u64) {
        let Some(step) = self.step else { return };
        let total = total.max(1);
        let finished = finished.min(total);
        let piece = piece.min(total - finished);
        if finished > self.clock_done {
            self.clock_done = finished;
            self.t0 = Instant::now();
        }
        let weight = self.weights.get(step).copied().unwrap_or(1);
        let paced = pacing(self.t0.elapsed().as_secs(), weight * piece / total) as u64;
        self.move_to(((self.clock_done * 10000 + paced * piece) / total) as i64);
    }

    /// What the window and the tray icon read: percent, label, done, total,
    /// paused and the detail, one per line. Labels are msgids, so each reader
    /// shows them in its own language.
    fn write_status(&self) {
        let label = if self.note.is_empty() { &self.label } else { &self.note };
        let text = format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n",
            self.shown.max(0),
            label,
            self.done_items,
            self.total,
            u8::from(self.paused),
            self.detail
        );
        let path = paths().progress();
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

/// How far work is that cannot be counted, in hundredths of a percent. It
/// moves at an even pace and gets to 90% when the expected time is up. From
/// there it slows down and never arrives. A bar that stands still looks like
/// a hang, and somebody who reads it that way reaches for the power button in
/// the middle of an update.
pub fn pacing(seconds: u64, expected: u64) -> i64 {
    let (s, e) = (seconds as i64, expected.max(1) as i64);
    if s <= e { 9000 * s / e } else { 9000 + 1000 * (s - e) / s }
}

/// A line to one helper. Never blocks: a helper that stopped reading loses
/// the line rather than holding up the update.
fn write_line(s: &mut Session, line: &str) {
    let Some(stdin) = s.stdin.as_mut() else { return };
    let mut buf = line.as_bytes().to_vec();
    buf.push(b'\n');
    match stdin.write(&buf) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
        Err(_) => s.stdin = None,
    }
}

/// The last lines of a file, each cut to a width.
fn tail(path: &std::path::Path) -> String {
    let Ok(mut f) = std::fs::File::open(path) else { return String::new() };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let _ = f.seek(SeekFrom::Start(len.saturating_sub(16384)));
    let mut buf = Vec::new();
    let _ = f.read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf).replace('\r', "\n");
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let skip = lines.len().saturating_sub(TAIL_LINES);
    lines[skip..].iter().map(|l| l.chars().take(TAIL_COLS).collect::<String>()).collect::<Vec<_>>().join("\n")
}

/// Applies MinimizeProgress. Plasma cannot open one job hidden, only turn off
/// the popups of a whole application, so this sets "Show popups" for our
/// desktop entry. Our other notifications carry no desktop entry, so only the
/// bar is affected. Written only when it differs.
fn popups(cfg: &Config, by_hand: bool, user: &User) {
    let want = if cfg.minimize_progress && !by_hand { "false" } else { "true" };
    let key = ["--file", "plasmanotifyrc", "--group", "Applications", "--group", "cachy-auto-update", "--key", "ShowPopups"];
    let Some(have) = proc::output(users::as_user(user, "kreadconfig6").args(key).args(["--default", "true"]), Duration::from_secs(10))
    else {
        return;
    };
    if !have.ok() || have.stdout.trim() == want {
        return;
    }
    // --notify, so a running desktop picks it up at once.
    let mut cmd = users::as_user(user, "kwriteconfig6");
    cmd.args(key);
    if want == "true" {
        cmd.args(["--delete", "--notify"]);
    } else {
        cmd.args(["--notify", "false"]);
    }
    proc::quiet(&mut cmd);
}

/// Starts the helper in a session and waits for it to say the entry is
/// open. Clicks it reports go to the control thread.
fn open_helper(user: &User, control: &Control) -> Option<Session> {
    let mut cmd = users::as_user(user, self_exe());
    cmd.args(["_progress", "--controls"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    let mut child = cmd.spawn().ok()?;
    let stdin = child.stdin.take()?;
    let stdout = child.stdout.take()?;
    // Never block on a helper: see write_line.
    unsafe {
        let fd = stdin.as_raw_fd();
        let flags = libc::fcntl(fd, libc::F_GETFL);
        libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
    }
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let clicks = control.sender();
    std::thread::spawn(move || {
        let mut lines = BufReader::new(stdout).lines();
        let first = lines.next().and_then(Result::ok);
        let _ = ready_tx.send(first.as_deref() == Some("ready"));
        for line in lines.map_while(Result::ok) {
            let _ = clicks.send(line);
        }
    });
    if ready_rx.recv_timeout(Duration::from_secs(5)) != Ok(true) {
        let _ = child.kill();
        let _ = child.wait();
        log::debug(format!("No progress bar for {}", user.name));
        return None;
    }
    Some(Session { child, stdin: Some(stdin), locale: users::locale_of(user) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar(steps: &[(&'static str, u64)]) -> Progress {
        let dir = std::env::temp_dir().join(format!("cau-progress-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        unsafe { std::env::set_var("CAU_RUNDIR", &dir) };
        let p = Progress::new();
        {
            let mut i = p.inner.lock().unwrap();
            i.plan = steps.iter().map(|(s, _)| *s).collect();
            i.weights = steps.iter().copied().collect();
        }
        p
    }

    fn pos(p: &Progress) -> i64 {
        p.inner.lock().unwrap().pos
    }

    #[test]
    fn steps_share_the_bar_by_weight() {
        let p = bar(&[("a", 10), ("b", 30)]);
        p.step("a", "A", 0);
        p.at(5, 10);
        assert_eq!(pos(&p), 1250);
        // Never backwards.
        p.at(1, 10);
        assert_eq!(pos(&p), 1250);
        p.step("b", "B", 0);
        p.at(10, 10);
        assert_eq!(pos(&p), 10000);
    }

    #[test]
    fn dropping_a_step_does_not_move_the_bar() {
        let p = bar(&[("a", 10), ("b", 10), ("c", 20)]);
        p.step("a", "A", 0);
        p.at(1, 1);
        let before = pos(&p);
        p.drop_steps(&["c"]);
        assert_eq!(pos(&p), before);
        p.step("b", "B", 0);
        p.at(1, 1);
        assert_eq!(pos(&p), 10000);
        // A dropped step is not one to move to.
        p.step("c", "C", 0);
        assert_eq!(p.inner.lock().unwrap().step, Some("b"));
    }

    #[test]
    fn the_clock_slows_down_past_the_estimate() {
        assert_eq!(pacing(0, 10), 0);
        assert_eq!(pacing(10, 10), 9000);
        assert_eq!(pacing(20, 10), 9500);
        assert!(pacing(1000, 10) < 10000);
    }
}
