//! Every piece of state is one small file under the state folder. The CLI, the
//! update run, the window and the tray icon all read the same files.

use crate::paths::paths;

pub fn read(key: &str) -> Option<String> {
    std::fs::read_to_string(paths().state.join(key)).ok().map(|s| s.trim_end_matches('\n').to_string())
}

pub fn read_or(key: &str, default: &str) -> String {
    read(key).unwrap_or_else(|| default.to_string())
}

/// A number, or 0.
pub fn number(key: &str) -> u64 {
    read(key).and_then(|s| s.trim().parse().ok()).unwrap_or(0)
}

pub fn write(key: &str, value: &str) -> std::io::Result<()> {
    let dir = &paths().state;
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join(key), format!("{value}\n"))
}

pub fn clear(key: &str) {
    let _ = std::fs::remove_file(paths().state.join(key));
}

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}
