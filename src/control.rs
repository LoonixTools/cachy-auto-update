//! Cancel and pause, from the buttons on the desktop's progress entry.
//!
//! A progress helper reports a click as one line: "cancel", "pause" or
//! "resume". They land here, in a thread that runs next to the update.
//!
//! Nothing is ever cut off while it installs. Every long command runs through
//! `Control::run`, which records what kind of command it is, and the thread
//! only stops it where that is safe:
//!
//!   pacman   while it syncs and downloads, never once it checks or installs
//!   aur      while paru builds, never while it holds pacman's lock
//!   flatpak  any time, a deploy is atomic
//!   other    never, the run stops or pauses once it is done
//!
//! Pause stops the command the same way. On resume the step runs again and
//! picks up what is already downloaded.

use crate::config::Config;
use crate::notify::{self, Note, Urgency};
use crate::paths::PACMAN_LOCK;
use crate::progress::Progress;
use crate::{log, proc};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Pacman,
    Aur,
    Flatpak,
    Other,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Pacman => "pacman",
            Kind::Aur => "aur",
            Kind::Flatpak => "flatpak",
            Kind::Other => "other",
        }
    }
}

struct Work {
    kind: Kind,
    pid: i32,
    /// pacman's output so far, to see whether it got to installing.
    note: Option<PathBuf>,
}

#[derive(Default)]
struct Flags {
    cancel: bool,
    pause: bool,
    /// The last command was stopped by a click rather than ending by itself.
    stopped: bool,
    work: Option<Work>,
}

#[derive(Clone)]
pub struct Control {
    flags: Arc<Mutex<Flags>>,
    tx: Sender<String>,
    rx: Arc<Mutex<Option<Receiver<String>>>>,
    stop: Arc<AtomicBool>,
    thread: Arc<Mutex<Option<std::thread::JoinHandle<()>>>>,
}

/// What `interrupt` did.
#[derive(PartialEq)]
enum Interrupt {
    Stopped,
    NotSafe,
    Nothing,
}

impl Default for Control {
    fn default() -> Self {
        Self::new()
    }
}

impl Control {
    pub fn new() -> Self {
        let (tx, rx) = channel();
        Control {
            flags: Arc::default(),
            tx,
            rx: Arc::new(Mutex::new(Some(rx))),
            stop: Arc::default(),
            thread: Arc::default(),
        }
    }

    /// Where helpers' clicks go.
    pub fn sender(&self) -> Sender<String> {
        self.tx.clone()
    }

    pub fn cancelled(&self) -> bool {
        self.flags.lock().unwrap().cancel
    }

    pub fn stopped(&self) -> bool {
        self.flags.lock().unwrap().stopped
    }

    pub fn paused(&self) -> bool {
        self.flags.lock().unwrap().pause
    }

    /// Waits as long as the run is paused. False once it is cancelled.
    pub fn wait(&self, progress: &Progress) -> bool {
        if self.cancelled() {
            return false;
        }
        if !self.paused() {
            return true;
        }
        log::info("Paused");
        progress.suspended(true);
        while self.paused() && !self.cancelled() {
            std::thread::sleep(Duration::from_millis(500));
        }
        if self.cancelled() {
            return false;
        }
        log::info("Resumed");
        progress.suspended(false);
        true
    }

    /// Runs a long command where the clicks can find it, with its output in
    /// `log_to` (the last-run log when None). Not started while the run is
    /// paused, and not at all once it is cancelled: then None.
    pub fn run(
        &self,
        progress: &Progress,
        kind: Kind,
        note: Option<&Path>,
        cmd: &mut Command,
        log_to: Option<&Path>,
    ) -> Option<ExitStatus> {
        self.flags.lock().unwrap().stopped = false;
        if !self.wait(progress) {
            return None;
        }
        let logged = match log::spawn_into(cmd, log_to) {
            Ok(l) => l,
            Err(e) => {
                log::warn(format!("Could not start {:?}: {e}", cmd.get_program()));
                return Some(std::os::unix::process::ExitStatusExt::from_raw(127 << 8));
            }
        };
        self.flags.lock().unwrap().work =
            Some(Work { kind, pid: logged.child.id() as i32, note: note.map(Path::to_path_buf) });
        let status = logged.wait();
        self.flags.lock().unwrap().work = None;
        status.ok()
    }

    /// Starts the thread that acts on clicks. Only once the helpers run.
    pub fn start(&self, progress: Progress, cfg: Config) {
        let Some(rx) = self.rx.lock().unwrap().take() else { return };
        let me = self.clone();
        let handle = std::thread::spawn(move || {
            while !me.stop.load(Ordering::Relaxed) {
                match rx.recv_timeout(Duration::from_secs(1)) {
                    // Only these three words: the line comes from a user's process.
                    Ok(line) => match line.trim() {
                        "cancel" => me.cancel(&progress, &cfg),
                        "pause" => me.pause(&progress),
                        "resume" => me.resume(&progress),
                        _ => {}
                    },
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        // Asked for but not safe a moment ago. paru holds pacman's
                        // lock only while it installs, so a few seconds later it
                        // usually is.
                        let wanted = {
                            let f = me.flags.lock().unwrap();
                            f.cancel || f.pause
                        };
                        if wanted {
                            me.interrupt();
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        *self.thread.lock().unwrap() = Some(handle);
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.lock().unwrap().take() {
            let _ = t.join();
        }
    }

    fn cancel(&self, progress: &Progress, cfg: &Config) {
        {
            let mut f = self.flags.lock().unwrap();
            if f.cancel {
                return;
            }
            f.cancel = true;
            f.pause = false;
        }
        log::info("Cancel requested from the desktop");
        if self.interrupt() != Interrupt::NotSafe {
            return;
        }
        // The desktop drops the progress entry two seconds after the click, so
        // a step that has to finish first is announced with a notification.
        log::info("Stopping once the current step is done");
        notify::notify_tagged(
            cfg,
            Some("run"),
            false,
            Note {
                urgency: Urgency::Normal,
                linger: false,
                title: "Stopping the update",
                body: "The current step is finished first. Please leave the computer switched on until then.",
                args: vec![],
            },
        );
        let _ = progress;
    }

    fn pause(&self, progress: &Progress) {
        {
            let mut f = self.flags.lock().unwrap();
            if f.cancel || f.pause {
                return;
            }
            f.pause = true;
        }
        log::info("Pause requested from the desktop");
        if self.interrupt() == Interrupt::NotSafe {
            progress.note("Pausing once the current step is done");
        }
    }

    fn resume(&self, progress: &Progress) {
        {
            let mut f = self.flags.lock().unwrap();
            if !f.pause {
                return;
            }
            f.pause = false;
        }
        // In case it never got as far as pausing.
        progress.note("");
    }

    /// Stops what `run` runs, if that is safe right now.
    fn interrupt(&self) -> Interrupt {
        let (kind, pid, note) = {
            let f = self.flags.lock().unwrap();
            if f.stopped {
                return Interrupt::Stopped;
            }
            match &f.work {
                Some(w) => (w.kind, w.pid, w.note.clone()),
                None => return Interrupt::Nothing,
            }
        };
        if !proc::alive(pid) {
            return Interrupt::Nothing;
        }
        let tree = proc::tree(&proc::processes(), pid);
        match kind {
            Kind::Pacman => {
                // Once the downloads are done pacman checks and then installs. A
                // signal from there on could leave half an upgrade behind.
                let text = note.and_then(|n| std::fs::read(n).ok()).unwrap_or_default();
                if crate::pkg::pacman::committed(&String::from_utf8_lossy(&text)) {
                    return Interrupt::NotSafe;
                }
                // Marked first: the run looks for it as soon as pacman exits.
                // SIGINT, not SIGTERM: pacman catches it, ends the download and
                // releases its lock.
                self.flags.lock().unwrap().stopped = true;
                for p in &tree {
                    let comm = std::fs::read_to_string(format!("/proc/{p}/comm")).unwrap_or_default();
                    if comm.trim() == "pacman" {
                        proc::signal(*p, libc::SIGINT);
                    }
                }
            }
            Kind::Aur => {
                if Path::new(PACMAN_LOCK).exists() {
                    return Interrupt::NotSafe;
                }
                // Frozen before the second look at the lock, so paru cannot start
                // installing in between.
                tree.iter().for_each(|p| proc::signal(*p, libc::SIGSTOP));
                let tree = proc::tree(&proc::processes(), pid);
                tree.iter().for_each(|p| proc::signal(*p, libc::SIGSTOP));
                if !frozen(&tree) || Path::new(PACMAN_LOCK).exists() {
                    tree.iter().for_each(|p| proc::signal(*p, libc::SIGCONT));
                    return Interrupt::NotSafe;
                }
                self.flags.lock().unwrap().stopped = true;
                tree.iter().for_each(|p| proc::signal(*p, libc::SIGTERM));
                tree.iter().for_each(|p| proc::signal(*p, libc::SIGCONT));
            }
            Kind::Flatpak => {
                self.flags.lock().unwrap().stopped = true;
                tree.iter().for_each(|p| proc::signal(*p, libc::SIGTERM));
            }
            Kind::Other => return Interrupt::NotSafe,
        }
        log::info(format!("Stopped the running {} step", kind.name()));
        Interrupt::Stopped
    }
}

/// Waits up to two seconds until all of them have really stopped. SIGSTOP
/// lands a moment later, and the lock check after it must not race a
/// process that still runs.
fn frozen(pids: &[i32]) -> bool {
    for _ in 0..40 {
        let busy = pids.iter().any(|p| proc::state_letter(*p).is_some_and(|s| !matches!(s, 'T' | 't' | 'Z' | 'X')));
        if !busy {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}
