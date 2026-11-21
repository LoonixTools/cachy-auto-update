//! The window's state and what its buttons do.

use crate::desktop::{Cmd, Look};
use cachy_auto_update::config::{self, Config};
use cachy_auto_update::i18n::{tr, trf};
use cachy_auto_update::paths::NOW_UNIT;
use cachy_auto_update::reboot::Then;
use cachy_auto_update::status::{LastResult, Next, RunProgress, Status};
use cachy_auto_update::{check, state, status};
use rune::{AppHandle, TextEdit};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::Sender;
use std::time::Duration;

/// How often to look for updates, like cachy-update: two minutes after login,
/// once the desktop settled, then once a day. A look that got no answer
/// (offline) is tried again sooner.
const CHECK_EVERY: u64 = 86400;
const FIRST_CHECK: u64 = 120;
const CHECK_RETRY: u64 = 300;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Pane {
    #[default]
    Overview,
    Schedule,
    Updates,
    Notifications,
    Cleanup,
    Language,
    Log,
}

impl Pane {
    pub const ALL: [Pane; 7] =
        [Pane::Overview, Pane::Schedule, Pane::Updates, Pane::Notifications, Pane::Cleanup, Pane::Language, Pane::Log];

    pub fn title(self) -> &'static str {
        match self {
            Pane::Overview => tr("Overview"),
            Pane::Schedule => tr("Schedule"),
            Pane::Updates => tr("What to update"),
            Pane::Notifications => tr("Notifications"),
            Pane::Cleanup => tr("Cleanup"),
            Pane::Language => tr("Language"),
            Pane::Log => tr("Log"),
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Pane::Overview => "nav-overview",
            Pane::Schedule => "nav-schedule",
            Pane::Updates => "nav-updates",
            Pane::Notifications => "nav-notifications",
            Pane::Cleanup => "nav-cleanup",
            Pane::Language => "nav-language",
            Pane::Log => "nav-log",
        }
    }

    pub fn icon(self) -> &'static rune::Icon {
        use crate::icons::*;
        match self {
            Pane::Overview => &HOME,
            Pane::Schedule => &CLOCK,
            Pane::Updates => &PACKAGE,
            Pane::Notifications => &BELL,
            Pane::Cleanup => &BROOM,
            Pane::Language => &GLOBE,
            Pane::Log => &LOG,
        }
    }

    /// For `--pane` and the D-Bus Show call.
    pub fn from_name(name: &str) -> Pane {
        match name {
            "schedule" => Pane::Schedule,
            "updates" => Pane::Updates,
            "notifications" => Pane::Notifications,
            "cleanup" => Pane::Cleanup,
            "language" => Pane::Language,
            "log" => Pane::Log,
            _ => Pane::Overview,
        }
    }
}

/// Why an update would wait right now.
#[derive(Clone, Debug, Default)]
pub struct Conditions {
    pub ac: bool,
    pub battery: Option<i64>,
    pub busy: Option<String>,
    pub package_manager: Option<String>,
}

/// What a background thread read from the files the update run writes.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub cfg: Config,
    pub status: Status,
    pub running: bool,
    pub progress: Option<RunProgress>,
    /// The running update shuts down or restarts the computer when done.
    pub then: Option<Then>,
}

impl Snapshot {
    pub fn read() -> Self {
        let running = status::running().is_some();
        Snapshot {
            cfg: Config::load(),
            status: Status::read(),
            running,
            progress: if running { status::run_progress() } else { None },
            then: if running { cachy_auto_update::reboot::announced() } else { None },
        }
    }
}

/// What the background threads share with the window.
#[derive(Default)]
pub struct Shared {
    /// Which log the Log pane shows: 0 none, 1 the last run, 2 all runs.
    pub log: AtomicU8,
}

/// The running app around the state: none in headless scenes and tests.
#[derive(Clone)]
pub struct Host {
    pub app: AppHandle<Ui>,
    pub desktop: Option<Sender<Cmd>>,
    pub shared: Arc<Shared>,
    /// Started at login with no window: the first look for updates waits
    /// until the desktop settled.
    pub at_login: bool,
}

pub struct Ui {
    pub pane: Pane,
    pub cfg: Config,
    /// Values on their way into the file, shown at once.
    pub saving: HashMap<&'static str, String>,
    pub status: Status,
    pub running: bool,
    pub progress: Option<RunProgress>,
    /// Update now was clicked and the run has not begun yet.
    pub starting: bool,
    /// What follows the update that runs or starts.
    pub then: Option<Then>,
    pub pending: Option<u64>,
    pub checking: bool,
    /// When the next look for updates is due (Unix time).
    pub check_at: u64,
    pub next: Next,
    pub conditions: Option<Conditions>,
    /// The select that is open.
    pub open: Option<&'static str>,
    pub ignore: TextEdit,
    pub log_all: bool,
    pub log: String,
    pub error: Option<String>,
    pub host: Option<Host>,
    /// The tray icon is shown.
    pub tray: bool,
    /// The language picked for the window, "" for the desktop's.
    pub language: String,
    /// The app's logo, for the sidebar. The cache stays as long as it.
    pub images: Option<rune::ImageCache>,
    pub logo: Option<rune::Image>,
    sent: Option<Look>,
}

impl Ui {
    pub fn new(host: Option<Host>, snap: Snapshot) -> Self {
        let first = if host.as_ref().is_some_and(|h| h.at_login) { FIRST_CHECK } else { 0 };
        let ignore = TextEdit::new(snap.cfg.get("IgnorePkg"));
        let mut ui = Ui {
            pane: Pane::Overview,
            cfg: snap.cfg,
            saving: HashMap::new(),
            status: snap.status,
            running: snap.running,
            progress: snap.progress,
            starting: false,
            then: snap.then,
            pending: None,
            checking: false,
            check_at: state::now() + first,
            next: Next::Unknown,
            conditions: None,
            open: None,
            ignore,
            log_all: false,
            log: String::new(),
            error: None,
            host,
            tray: false,
            language: cachy_auto_update::i18n::ui_language().unwrap_or_default(),
            images: None,
            logo: None,
            sent: None,
        };
        ui.sync_tray();
        ui
    }

    /// The value a setting shows: the one being saved, else the file's.
    pub fn value(&self, key: &str) -> String {
        self.saving.get(key).cloned().unwrap_or_else(|| self.cfg.get(key))
    }

    pub fn on(&self, key: &str) -> bool {
        config::is_true(&self.value(key))
    }

    pub fn busy(&self) -> bool {
        self.running || self.starting
    }

    /// Takes in what the files say now.
    pub fn apply(&mut self, snap: Snapshot) {
        let was_running = self.running;
        let ignore_before = self.cfg.get("IgnorePkg");
        self.cfg = snap.cfg;
        self.status = snap.status;
        self.running = snap.running;
        self.progress = snap.progress;
        if self.running {
            self.starting = false;
        }
        if self.running || !self.starting {
            self.then = snap.then;
        }
        // What is left after an update: most likely nothing.
        if was_running && !self.running {
            self.check_soon();
        }
        // A field nobody edits follows the file.
        if self.ignore.text == ignore_before {
            self.ignore = TextEdit::new(self.cfg.get("IgnorePkg"));
        }
        self.follow_tray_setting();
        self.sync_tray();
    }

    /// Writes a setting. Needs root, so it goes through pkexec, which asks for
    /// the password once and then keeps it for a few minutes.
    pub fn set(&mut self, key: &'static str, value: String) {
        self.open = None;
        if self.value(key) == value {
            return;
        }
        self.saving.insert(key, value.clone());
        let Some(host) = self.host.clone() else {
            // Headless: there is no file to write.
            self.cfg.raw.insert(key.to_string(), value);
            self.cfg = Config::from_raw(std::mem::take(&mut self.cfg.raw));
            self.saving.remove(key);
            return;
        };
        std::thread::spawn(move || {
            let ok = crate::actions::config_set(key, &value);
            let snap = Snapshot::read();
            host.app.update(move |s, _| {
                s.saving.remove(key);
                if let Err(e) = ok {
                    s.error = Some(e);
                }
                s.apply(snap);
            });
        });
    }

    /// Shows the window and the tray icon in `lang` ("" for the desktop's)
    /// at once, and keeps it for the next start.
    pub fn set_language(&mut self, lang: String) {
        self.open = None;
        if lang == self.language {
            return;
        }
        cachy_auto_update::i18n::set_ui_language(Some(&lang).filter(|l| !l.is_empty()).map(String::as_str));
        // Headless: there is no file to write.
        if self.host.is_some() && crate::language::save(&lang).is_err() {
            self.error = Some(tr("Could not save the language.").into());
        }
        self.language = lang;
        self.sync_tray();
    }

    pub fn toggle(&mut self, key: &'static str) {
        let v = if self.on(key) { "no" } else { "yes" };
        self.set(key, v.into());
    }

    pub fn update_now(&mut self) {
        self.update(None);
    }

    /// Asks systemd to start the update, and with `then` to shut down or
    /// restart after it. polkit lets the person at the computer do that
    /// without a password.
    pub fn update(&mut self, then: Option<Then>) {
        self.open = None;
        if self.busy() {
            return;
        }
        self.starting = true;
        self.then = then;
        self.error = None;
        self.sync_tray();
        let Some(host) = self.host.clone() else { return };
        let unit = then.map_or(NOW_UNIT.to_string(), Then::unit);
        std::thread::spawn(move || {
            let ok = cachy_auto_update::proc::output(
                std::process::Command::new("systemctl").args(["start", "--no-block", &unit]),
                Duration::from_secs(120),
            )
            .is_some_and(|o| o.ok());
            // A run that does not show up within a minute did not start.
            std::thread::sleep(Duration::from_secs(if ok { 60 } else { 0 }));
            host.app.update(move |s, _| {
                if !ok {
                    s.error = Some(tr("The update could not be started.").into());
                }
                s.starting = false;
                s.sync_tray();
            });
        });
    }

    /// Looks for updates now.
    pub fn check(&mut self) {
        if self.busy() || self.checking {
            return;
        }
        self.checking = true;
        self.check_at = state::now() + CHECK_EVERY;
        self.sync_tray();
        let Some(host) = self.host.clone() else { return };
        let cfg = self.cfg.clone();
        std::thread::spawn(move || {
            let pending = check::count_updates(&cfg);
            host.app.update(move |s, _| {
                s.checking = false;
                match pending {
                    Some(n) => s.pending = Some(n),
                    None => s.check_at = state::now() + CHECK_RETRY,
                }
                s.sync_tray();
            });
        });
    }

    /// Looks again in a moment.
    pub fn check_soon(&mut self) {
        self.check_at = self.check_at.min(state::now() + 10);
    }

    /// Called every second. By the wall clock, which keeps going while the
    /// computer sleeps, so on a laptop "once a day" stays once a day. True
    /// when something changed that the window shows.
    pub fn tick(&mut self) -> bool {
        if state::now() >= self.check_at && !self.busy() && !self.checking {
            self.check();
            return true;
        }
        false
    }

    pub fn show_pane(&mut self, pane: Pane) {
        self.pane = pane;
        self.open = None;
        if let Some(h) = &self.host {
            h.shared.log.store(if pane == Pane::Log { 1 + u8::from(self.log_all) } else { 0 }, Ordering::Relaxed);
        }
    }

    pub fn set_log_all(&mut self, all: bool) {
        self.log_all = all;
        self.log.clear();
        self.show_pane(Pane::Log);
    }

    /// The tray icon follows TrayIcon: switched on in the window, it shows
    /// here; switched off, it goes, and so does the process once its window
    /// is closed.
    fn follow_tray_setting(&mut self) {
        let Some(h) = &self.host else { return };
        let Some(d) = &h.desktop else { return };
        if self.cfg.tray && !self.tray {
            let _ = d.send(Cmd::StartTray);
            self.tray = true;
            self.sent = None;
        } else if !self.cfg.tray && self.tray {
            let _ = d.send(Cmd::StopTray);
            self.tray = false;
            if !h.app.is_visible() {
                h.app.quit();
            }
        }
    }

    /// What the tray icon shows, sent when it changed.
    pub fn sync_tray(&mut self) {
        let look = self.look();
        if self.sent.as_ref() == Some(&look) {
            return;
        }
        if let Some(d) = self.host.as_ref().and_then(|h| h.desktop.as_ref()) {
            let _ = d.send(Cmd::Look(look.clone()));
        }
        self.sent = Some(look);
    }

    fn look(&self) -> Look {
        let icon = if self.busy() {
            "update-busy"
        } else if self.pending.is_some_and(|n| n > 0) {
            "update-low"
        } else {
            "update-none"
        };
        let mut lines = vec![self.headline()];
        if self.busy()
            && let Some(t) = self.then
        {
            lines.push(then_text(t).into());
        }
        lines.push(trf("Last update: %s", &[&status::time_ago(&cachy_auto_update::i18n::ui_locale(), self.status.last_success)]));
        match self.next {
            Next::At(ts) => lines.push(trf("Next update: %s", &[&status::strftime(ts, "%a %x %H:%M")])),
            _ if !self.cfg.enabled => lines.push(tr("Automatic updates are off.").into()),
            _ => {}
        }
        if !self.busy() {
            if !self.status.held.is_empty() {
                lines.push(trf("Held back: %s", &[&self.status.held.join(" ")]));
            }
            if self.status.result == LastResult::Failed {
                lines.push(tr("The last update had a problem.").into());
            }
            if self.status.reboot {
                lines.push(tr("A restart is recommended to finish a kernel update.").into());
            } else if cachy_auto_update::reboot::will_happen(&self.cfg, self.status.restart_planned) {
                lines.push(tr("A restart is planned to finish the update.").into());
            }
        }
        Look { icon, body: lines.join("\n"), busy: self.busy(), checking: self.checking, language: self.language.clone() }
    }

    /// One line on how things stand.
    pub fn headline(&self) -> String {
        if self.busy() {
            if self.progress.as_ref().is_some_and(|p| p.paused) {
                return tr("The update is paused").into();
            }
            return tr("Installing updates").into();
        }
        match self.pending {
            None if self.checking => tr("Looking for updates...").into(),
            None => tr("Not checked for updates yet").into(),
            Some(0) => tr("Everything is up to date").into(),
            Some(1) => tr("1 update available").into(),
            Some(n) => trf("%d updates available", &[&n]),
        }
    }
}

/// What happens once the update is done.
pub fn then_text(t: Then) -> &'static str {
    match t {
        Then::PowerOff => tr("The computer shuts down when the update is done."),
        Then::Reboot => tr("The computer restarts when the update is done."),
    }
}
