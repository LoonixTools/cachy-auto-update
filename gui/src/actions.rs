//! Things the window asks of the system.

use cachy_auto_update::i18n::tr;
use cachy_auto_update::proc;
use std::process::{Command, Stdio};

/// The command line program, next to this one.
pub fn cli() -> std::path::PathBuf {
    cachy_auto_update::paths::self_exe().with_file_name("cachy-auto-update")
}

/// Writes a setting through `cachy-auto-update config set`, as root by way
/// of pkexec. The polkit policy keeps the password for a few minutes, so
/// only the first change asks.
pub fn config_set(key: &str, value: &str) -> Result<(), String> {
    let mut cmd = if proc::is_root() {
        Command::new(cli())
    } else {
        let mut c = Command::new("pkexec");
        c.arg(cli());
        c
    };
    cmd.args(["config", "set", key, value]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    let out = cmd.output().map_err(|_| tr("Could not save the setting.").to_string())?;
    match out.status.code() {
        Some(0) => Ok(()),
        // The password dialog was closed, or the password was wrong.
        Some(126 | 127) => Err(tr("The setting was not changed: no permission.").into()),
        _ => {
            let why = String::from_utf8_lossy(&out.stderr).trim().trim_start_matches('✘').trim().to_string();
            Err(if why.is_empty() { tr("Could not save the setting.").into() } else { why })
        }
    }
}

/// The last lines of a log, read off the UI thread.
pub fn read_log(all: bool) -> String {
    let p = cachy_auto_update::paths::paths();
    let path = if all { p.logfile() } else { p.runlog() };
    let Ok(bytes) = std::fs::read(&path) else { return String::new() };
    // The last 256 KB: enough for a long run, and the view stays quick.
    let start = bytes.len().saturating_sub(256 * 1024);
    let text = String::from_utf8_lossy(&bytes[start..]).replace('\r', "\n");
    let lines: Vec<&str> = text.lines().collect();
    let skip = lines.len().saturating_sub(2000);
    lines[skip..].join("\n")
}
