//! Running programs and looking at the ones that run.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The program's path on PATH, if it is there.
pub fn which(name: &str) -> Option<PathBuf> {
    if name.contains('/') {
        let p = PathBuf::from(name);
        return is_executable(&p).then_some(p);
    }
    let path = std::env::var_os("PATH").unwrap_or_else(|| "/usr/local/sbin:/usr/local/bin:/usr/bin".into());
    std::env::split_paths(&path).map(|d| d.join(name)).find(|p| is_executable(p))
}

fn is_executable(p: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

pub fn have(name: &str) -> bool {
    which(name).is_some()
}

pub fn is_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

/// What a finished program left behind.
pub struct Output {
    pub status: Option<i32>,
    pub stdout: String,
}

impl Output {
    pub fn ok(&self) -> bool {
        self.status == Some(0)
    }
}

/// Runs a program to its end and collects its output, killing it after
/// `timeout`. None when it could not start or ran out of time.
pub fn output(cmd: &mut Command, timeout: Duration) -> Option<Output> {
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let buf = reader.join().unwrap_or_default();
    let status = status?;
    Some(Output { status: status.code(), stdout: String::from_utf8_lossy(&buf).into_owned() })
}

/// `output` with a command line.
pub fn run(program: &str, args: &[&str], timeout: Duration) -> Option<Output> {
    output(Command::new(program).args(args), timeout)
}

/// Runs a program and says only whether it succeeded.
pub fn quiet(cmd: &mut Command) -> bool {
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

/// One running process.
#[derive(Clone, Debug)]
pub struct Process {
    pub pid: i32,
    pub ppid: i32,
    pub comm: String,
}

/// Every process, read from /proc/<pid>/status. /stat is avoided on purpose:
/// its name field can hold spaces and brackets.
pub fn processes() -> Vec<Process> {
    let Ok(dir) = std::fs::read_dir("/proc") else { return Vec::new() };
    let mut out = Vec::new();
    for entry in dir.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|s| s.parse::<i32>().ok()) else { continue };
        let Ok(status) = std::fs::read_to_string(format!("/proc/{pid}/status")) else { continue };
        let (mut comm, mut ppid) = (String::new(), 0);
        for line in status.lines() {
            if let Some(v) = line.strip_prefix("Name:") {
                comm = v.trim().to_string();
            } else if let Some(v) = line.strip_prefix("PPid:") {
                ppid = v.trim().parse().unwrap_or(0);
                break;
            }
        }
        out.push(Process { pid, ppid, comm });
    }
    out
}

/// Whether a process of exactly this name runs, kernel threads not counted.
/// Exact, because a part of a name matches too much: "reaper" is in the
/// kernel's own oom_reaper. The kernel keeps 15 bytes of a name.
pub fn running(procs: &[Process], name: &str) -> bool {
    named(procs, name).next().is_some()
}

/// The processes of that name, kernel threads left out.
pub fn named<'a>(procs: &'a [Process], name: &'a str) -> impl Iterator<Item = &'a Process> + 'a {
    let name = &name[..name.len().min(15)];
    procs.iter().filter(move |p| p.pid != 2 && p.ppid != 2 && p.comm == name)
}

/// A process's arguments, without the program itself. Empty when they cannot
/// be read.
pub fn args(pid: i32) -> Vec<String> {
    let Ok(raw) = std::fs::read(format!("/proc/{pid}/cmdline")) else { return Vec::new() };
    raw.split(|&b| b == 0).skip(1).filter(|a| !a.is_empty()).map(|a| String::from_utf8_lossy(a).into_owned()).collect()
}

/// The pid and every process below it, parents first.
pub fn tree(procs: &[Process], root: i32) -> Vec<i32> {
    let mut out = vec![root];
    let mut i = 0;
    while i < out.len() {
        let parent = out[i];
        out.extend(procs.iter().filter(|p| p.ppid == parent && p.pid != parent).map(|p| p.pid));
        i += 1;
    }
    out
}

pub fn alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    if unsafe { libc::kill(pid, 0) } == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

pub fn signal(pid: i32, sig: i32) {
    if pid > 0 {
        unsafe {
            libc::kill(pid, sig);
        }
    }
}

/// The process state letter from /proc/<pid>/stat: R, S, T, Z and so on.
pub fn state_letter(pid: i32) -> Option<char> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.trim_start().chars().next()
}

/// The time the machine booted, from /proc/stat.
pub fn boot_time() -> Option<u64> {
    std::fs::read_to_string("/proc/stat")
        .ok()?
        .lines()
        .find_map(|l| l.strip_prefix("btime "))
        .and_then(|v| v.trim().parse().ok())
}
