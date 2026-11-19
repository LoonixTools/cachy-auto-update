//! Keeps the window current: a thread that looks at the files the update run
//! writes and hands news to the window. Nothing is read in the window's
//! frames, and nothing is sent while nothing changed.

use crate::ui::{Conditions, Host, Snapshot};
use cachy_auto_update::paths::{PACMAN_LOCK, paths};
use cachy_auto_update::{conditions, config::Config, locks, status};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant, SystemTime};

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// What changes when anything the window shows changes.
fn fingerprint(files: &[PathBuf]) -> Vec<Option<SystemTime>> {
    files.iter().map(|f| mtime(f)).collect()
}

fn watched() -> Vec<PathBuf> {
    let p = paths();
    let mut f = vec![p.config.clone(), p.running(), p.progress()];
    for key in ["last_run", "last_success", "last_result", "last_counts", "reboot_needed", "reboot_pending", "held_back"] {
        f.push(p.state.join(key));
    }
    f
}

pub fn start(host: Host) {
    std::thread::Builder::new()
        .name("cau-worker".into())
        .spawn(move || run(host))
        .ok();
}

fn run(host: Host) {
    let files = watched();
    let mut seen = fingerprint(&files);
    let mut lock = Path::new(PACMAN_LOCK).exists();
    let mut log_seen: (u8, Option<SystemTime>) = (0, None);
    let (mut last_slow, mut last_conditions) = (None::<Instant>, None::<Instant>);
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let now = fingerprint(&files);
        if now != seen {
            seen = now;
            let snap = Snapshot::read();
            host.app.update(move |s, _| s.apply(snap));
        }

        // pacman's lock went away: somebody finished installing by hand, and
        // what waits has changed.
        let locked = Path::new(PACMAN_LOCK).exists();
        if lock && !locked {
            host.app.update(|s, _| s.check_soon());
        }
        lock = locked;

        let visible = host.app.is_visible();
        // The next run's time asks systemd; once a minute is plenty.
        if last_slow.is_none_or(|t| t.elapsed() >= Duration::from_secs(60)) {
            last_slow = Some(Instant::now());
            let cfg = Config::load();
            let next = status::next_update(&cfg, status::Status::read().last_success);
            // "2 hours ago" in the tray's tooltip moves on too.
            host.app.update(move |s, _| {
                s.next = next;
                s.tick();
                s.sync_tray();
            });
        } else {
            host.app.update_if(|s, _| s.tick());
        }

        // Why an update would wait: only while somebody looks.
        if visible && last_conditions.is_none_or(|t| t.elapsed() >= Duration::from_secs(15)) {
            last_conditions = Some(Instant::now());
            let c = Conditions {
                ac: conditions::on_ac(),
                battery: conditions::battery_percent(),
                busy: conditions::busy(),
                package_manager: locks::package_manager_busy(),
            };
            host.app.update(move |s, _| s.conditions = Some(c));
        }
        if !visible {
            last_conditions = None;
        }

        // The Log pane's text, when it shows and the file changed.
        let want = host.shared.log.load(Ordering::Relaxed);
        if want > 0 && visible {
            let file = if want == 2 { paths().logfile() } else { paths().runlog() };
            let m = mtime(&file);
            if (want, m) != log_seen {
                log_seen = (want, m);
                let text = crate::actions::read_log(want == 2);
                host.app.update(move |s, _| {
                    if 1 + u8::from(s.log_all) == want {
                        s.log = text;
                    }
                });
            }
        } else {
            log_seen = (0, None);
        }
    }
}
