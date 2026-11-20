//! Where everything lives. Each folder can be moved with an environment
//! variable, so the code can be tried out without installing it.

use std::path::PathBuf;
use std::sync::OnceLock;

pub const NAME: &str = "cachy-auto-update";
pub const PRETTY: &str = "CachyOS Auto-Update";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The locked system account that builds and installs AUR packages.
pub const BUILD_USER: &str = "cachy-auto-update";

/// pacman's own database lock. It exists while somebody is in a transaction.
pub const PACMAN_LOCK: &str = "/var/lib/pacman/db.lck";

/// The systemd units.
pub const TIMER: &str = "cachy-auto-update.timer";
pub const NOW_UNIT: &str = "cachy-auto-update-now.service";

pub struct Paths {
    pub config: PathBuf,
    pub state: PathBuf,
    pub cache: PathBuf,
    pub log: PathBuf,
    pub run: PathBuf,
}

fn dir(var: &str, default: &str) -> PathBuf {
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| default.into())
}

pub fn paths() -> &'static Paths {
    static P: OnceLock<Paths> = OnceLock::new();
    P.get_or_init(|| {
        let confdir = dir("CAU_CONFDIR", "/etc/cachy-auto-update");
        Paths {
            config: std::env::var_os("CAU_CONFIG")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| confdir.join("cachy-auto-update.conf")),
            state: dir("CAU_STATEDIR", "/var/lib/cachy-auto-update"),
            cache: dir("CAU_CACHEDIR", "/var/cache/cachy-auto-update"),
            log: dir("CAU_LOGDIR", "/var/log/cachy-auto-update"),
            run: dir("CAU_RUNDIR", "/run/cachy-auto-update"),
        }
    })
}

impl Paths {
    /// Every run, appended.
    pub fn logfile(&self) -> PathBuf {
        self.log.join("cachy-auto-update.log")
    }
    /// The last run that really updated.
    pub fn runlog(&self) -> PathBuf {
        self.log.join("last-run.log")
    }
    pub fn lockfile(&self) -> PathBuf {
        self.run.join("run.lock")
    }
    /// Holds the pid of a running update.
    pub fn running(&self) -> PathBuf {
        self.run.join("running")
    }
    /// How far a running update is, for the window and the tray icon.
    pub fn progress(&self) -> PathBuf {
        self.run.join("progress")
    }
    /// What follows a running update: poweroff or reboot.
    pub fn then(&self) -> PathBuf {
        self.run.join("then")
    }
    pub fn notify_queue(&self) -> PathBuf {
        self.state.join("notify-queue")
    }
    /// Home of the build account.
    pub fn build_home(&self) -> PathBuf {
        self.state.join("builder")
    }
}

/// This program, for running it again as another user. An update can replace
/// the binary while it runs; then the new one at the same path is meant.
pub fn self_exe() -> PathBuf {
    match std::env::current_exe() {
        Ok(p) => {
            let s = p.to_string_lossy();
            match s.strip_suffix(" (deleted)") {
                Some(rest) => PathBuf::from(rest),
                None => p,
            }
        }
        Err(_) => PathBuf::from("/usr/bin/cachy-auto-update"),
    }
}
