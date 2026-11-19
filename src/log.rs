//! The log. Lines go to stderr (so journald has them for the service) and,
//! once the files are open, to the log of every run and of the last run.

use crate::paths::paths;
use std::fs::{File, OpenOptions};
use std::io::{IsTerminal, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

static OPEN: AtomicBool = AtomicBool::new(false);
static DEBUG: AtomicBool = AtomicBool::new(false);
static QUIET: AtomicBool = AtomicBool::new(false);

/// Whether a person watches stdout. Decided once at start.
pub fn interactive() -> bool {
    static I: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *I.get_or_init(|| std::io::stdout().is_terminal())
}

pub fn set_debug(on: bool) {
    DEBUG.store(on, Ordering::Relaxed);
}

/// No lines at all, for work done only to feed the progress bar.
pub fn set_quiet(on: bool) {
    QUIET.store(on, Ordering::Relaxed);
}

fn append(path: &std::path::Path, line: &str) {
    if let Ok(mut f) = OpenOptions::new().append(true).create(true).open(path) {
        let _ = f.write_all(line.as_bytes());
    }
}

pub fn timestamp() -> String {
    // localtime_r through libc: the log reads in local time, as before.
    let now = unsafe { libc::time(std::ptr::null_mut()) };
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&now, &mut tm) };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

pub fn log(level: &str, msg: &str) {
    if QUIET.load(Ordering::Relaxed) {
        return;
    }
    let line = format!("{} [{level}] {msg}\n", timestamp());
    let _ = std::io::stderr().write_all(line.as_bytes());
    if OPEN.load(Ordering::Relaxed) {
        append(&paths().logfile(), &line);
        append(&paths().runlog(), &line);
    }
}

pub fn info(msg: impl AsRef<str>) {
    log("INFO", msg.as_ref());
}
pub fn warn(msg: impl AsRef<str>) {
    log("WARN", msg.as_ref());
}
pub fn error(msg: impl AsRef<str>) {
    log("ERROR", msg.as_ref());
}
pub fn debug(msg: impl AsRef<str>) {
    if DEBUG.load(Ordering::Relaxed) {
        log("DEBUG", msg.as_ref());
    }
}

/// Opens the log files for this run, as root. The last-run log is emptied
/// only once a run really starts updating (`reset_runlog`): the timer fires
/// every hour and most of those runs do nothing. Readable for everyone, like
/// pacman's own log.
pub fn open() {
    let dir = &paths().log;
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755));
    if OpenOptions::new().append(true).create(true).open(paths().runlog()).is_ok() {
        OPEN.store(true, Ordering::Relaxed);
    }
}

pub fn reset_runlog() {
    if OPEN.load(Ordering::Relaxed) {
        let _ = File::create(paths().runlog());
    }
}

/// Appends text to the last-run log.
pub fn append_runlog(text: &str) {
    if OPEN.load(Ordering::Relaxed) {
        append(&paths().runlog(), text);
    }
}

/// A program started with its output in a log file, and on the terminal
/// too while a person watches.
pub struct Logged {
    pub child: std::process::Child,
    tee: Option<std::thread::JoinHandle<()>>,
}

impl Logged {
    pub fn wait(mut self) -> std::io::Result<std::process::ExitStatus> {
        let status = self.child.wait();
        if let Some(t) = self.tee.take() {
            let _ = t.join();
        }
        status
    }
}

/// Starts `cmd` with stdout and stderr appended to `file` (the last-run log
/// when None). Without open logs the output goes to stderr instead.
pub fn spawn_into(cmd: &mut Command, file: Option<&std::path::Path>) -> std::io::Result<Logged> {
    cmd.stdin(Stdio::null());
    let path = match file {
        Some(p) => p.to_path_buf(),
        None if OPEN.load(Ordering::Relaxed) => paths().runlog(),
        None => {
            use std::os::fd::AsFd;
            let err = std::io::stderr().as_fd().try_clone_to_owned()?;
            cmd.stdout(Stdio::from(err)).stderr(Stdio::inherit());
            return Ok(Logged { child: cmd.spawn()?, tee: None });
        }
    };
    if !interactive() {
        let f = OpenOptions::new().append(true).create(true).open(&path)?;
        let f2 = f.try_clone()?;
        cmd.stdout(Stdio::from(f)).stderr(Stdio::from(f2));
        return Ok(Logged { child: cmd.spawn()?, tee: None });
    }
    // One pipe for both, so the lines stay in order.
    let (mut r, w) = pipe()?;
    let w2 = w.try_clone()?;
    cmd.stdout(Stdio::from(w)).stderr(Stdio::from(w2));
    let child = cmd.spawn();
    // Our copies of the write end go now, else the copy never sees the end.
    cmd.stdout(Stdio::null()).stderr(Stdio::null());
    let child = child?;
    let tee = std::thread::spawn(move || {
        let mut log = OpenOptions::new().append(true).create(true).open(&path).ok();
        let mut buf = [0u8; 8192];
        loop {
            match r.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let _ = std::io::stdout().write_all(&buf[..n]);
                    let _ = std::io::stdout().flush();
                    if let Some(l) = log.as_mut() {
                        let _ = l.write_all(&buf[..n]);
                    }
                }
            }
        }
    });
    Ok(Logged { child, tee: Some(tee) })
}

/// Runs a command with its output in the last-run log. True on success.
pub fn run_logged(cmd: &mut Command) -> bool {
    match spawn_into(cmd, None) {
        Ok(l) => l.wait().is_ok_and(|s| s.success()),
        Err(e) => {
            warn(format!("Could not start {:?}: {e}", cmd.get_program()));
            false
        }
    }
}

pub fn pipe() -> std::io::Result<(File, File)> {
    use std::os::fd::FromRawFd;
    let mut fds = [0; 2];
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1])) })
}
