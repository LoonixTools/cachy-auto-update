//! cachy-auto-update-gui: the window and the tray icon, drawn with Rune.
//!
//! One copy per session. Started by the autostart with `--tray`, it sits in
//! the tray with no window; a click on the icon opens the window. Started from
//! the menu, it opens the window, and a second start shows the first copy's
//! window instead of opening another.

mod actions;
mod controls;
mod desktop;
mod icons;
mod log_pane;
mod overview;
mod settings;
mod ui;
mod view;
mod worker;

use cachy_auto_update::config::Config;
use cachy_auto_update::paths::PRETTY;
use cachy_auto_update::state;
use cachy_auto_update::status::{self, LastResult, Next, RunProgress, Status};
use rune::{App, AppSpec, Tx};
use std::sync::Arc;
use ui::{Conditions, Host, Pane, Shared, Snapshot, Ui};

/// Rune's own headless flags: render, no bus, no tray, no threads.
const HEADLESS: &[&str] = &["--snap", "--tree", "--script", "--video", "--list-scenes", "--out", "--trace", "--strip"];

/// The same made-up machine for every scene, so pictures do not depend on
/// this one.
fn scene_base() -> Ui {
    let now = state::now();
    let mut raw = std::collections::HashMap::new();
    raw.insert("Enabled".to_string(), "yes".to_string());
    let snap = Snapshot {
        cfg: Config::from_raw(raw),
        status: Status {
            last_run: now - 1800,
            last_success: now - 2 * 86400 - 3600,
            result: LastResult::Ok,
            counts: [127, 3, 2, 0],
            reboot: false,
            restart_planned: false,
            held: vec![],
        },
        running: false,
        progress: None,
    };
    let mut ui = Ui::new(None, snap);
    ui.pending = Some(0);
    ui.next = Next::At(now + 5 * 86400 - 3600);
    ui.conditions = Some(Conditions { ac: true, battery: Some(84), busy: None, package_manager: None });
    ui.log = "2026-11-16 09:12:01 [INFO] Starting update run\n2026-11-16 09:12:09 [INFO] Updating 127 repository package(s)\n:: Synchronizing package databases...\n:: Starting full system upgrade...\n( 1/127) upgrading glibc\n( 2/127) upgrading linux-cachyos\n2026-11-16 09:16:39 [INFO] Update run finished successfully (132 item(s) updated)".into();
    ui
}

type Scene = (&'static str, fn(&mut Ui));

const SCENES: &[Scene] = &[
    ("uptodate", |_| {}),
    ("updates", |s| s.pending = Some(12)),
    ("running", |s| {
        s.running = true;
        s.progress = Some(RunProgress {
            percent: 42,
            label: "Updating system packages".into(),
            done: 87,
            total: 214,
            paused: false,
            detail: "linux-cachyos".into(),
        });
    }),
    ("problem", |s| {
        s.status.result = LastResult::Failed;
        s.status.reboot = true;
        s.status.held = vec!["heidisql-qt6-bin".into()];
        s.pending = Some(1);
        s.conditions = Some(Conditions {
            ac: false,
            battery: Some(22),
            busy: Some("a game is running (gamescope)".into()),
            package_manager: None,
        });
    }),
    ("planned", |s| {
        s.status.restart_planned = true;
        s.cfg.raw.insert("AutoReboot".into(), "always".into());
        s.cfg = Config::from_raw(std::mem::take(&mut s.cfg.raw));
    }),
    ("schedule", |s| s.pane = Pane::Schedule),
    ("restart", |s| {
        s.pane = Pane::Schedule;
        s.cfg.raw.insert("AutoReboot".into(), "needed".into());
        s.cfg = Config::from_raw(std::mem::take(&mut s.cfg.raw));
    }),
    ("what", |s| s.pane = Pane::Updates),
    ("notifications", |s| s.pane = Pane::Notifications),
    ("cleanup", |s| s.pane = Pane::Cleanup),
    ("log", |s| s.pane = Pane::Log),
];

/// The logo, the same file the menu entry shows.
const LOGO: &[u8] = include_bytes!("../../res/cachy-auto-update.svg");

/// Draws the logo once, at twice the size it shows at.
fn setup(s: &mut Ui, waker: rune::Waker) {
    let cache = rune::ImageCache::new(waker);
    let logo = cache.svg(LOGO, 32.0, 2.0);
    // A few milliseconds, and the first frame has it.
    let _ = logo.wait();
    s.logo = Some(logo);
    s.images = Some(cache);
}

fn init_default() -> Ui {
    Ui::new(None, Snapshot::read())
}

/// With the tray icon, closing the window only hides it.
fn on_close(s: &mut Ui, tx: &mut Tx) {
    s.open = None;
    if !s.tray {
        tx.quit();
    }
}

fn main() {
    status::init_time_locale();
    // Wayland's app_id and X11's WM_CLASS: the menu entry of the same name
    // gives the window its icon and name in the task bar.
    rune::set_app_id("cachy-auto-update");
    let args: Vec<String> = std::env::args().skip(1).collect();
    let tray_start = args.iter().any(|a| a == "--tray");
    let pane = args.windows(2).find(|w| w[0] == "--pane").map(|w| Pane::from_name(&w[1])).unwrap_or_default();
    let mut rune_args = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--tray" => {}
            "--pane" => {
                it.next();
            }
            _ => rune_args.push(a.clone()),
        }
    }
    let headless = rune_args.iter().any(|a| HEADLESS.contains(&a.as_str()));

    let spec = AppSpec::<Ui> {
        fonts: rune_theme_opencode::FONTS,
        families: rune_theme_opencode::FAMILIES,
        min_size: Some((760.0, 560.0)),
        scenes: SCENES,
        setup: Some(setup),
        ..AppSpec::new(PRETTY, (920.0, 700.0), init_default, view::view)
    };

    if headless {
        App::new(spec).args(rune_args).init(scene_base).run();
        return;
    }

    let cfg = Config::load();
    // The autostart, with the icon switched off: nothing to do.
    if tray_start && !cfg.tray {
        return;
    }
    let handle: desktop::Handle = Arc::default();
    let desktop_tx = match desktop::start(handle.clone(), pane, !tray_start) {
        desktop::Claim::First(tx) => Some(tx),
        // Another copy runs; it shows its window.
        desktop::Claim::Other => return,
        desktop::Claim::NoBus => None,
    };
    let tray_mode = cfg.tray && desktop_tx.is_some();

    // The process outlives its window: whether closing it quits depends on
    // the tray icon, which can be switched on and off while it runs.
    let mut app = App::new(spec).args(rune_args).on_close(on_close).keep_running().release_gpu_when_closed();
    if tray_start {
        app = app.windowless();
    }
    let host = Host {
        app: app.handle(),
        desktop: desktop_tx.clone(),
        shared: Arc::new(Shared::default()),
        at_login: tray_start,
    };
    let _ = handle.set(host.app.clone());
    let h = host.clone();
    app = app.init(move || {
        let mut ui = Ui::new(Some(h), Snapshot::read());
        ui.tray = tray_mode;
        ui.show_pane(pane);
        ui
    });
    if tray_mode && let Some(tx) = &desktop_tx {
        let _ = tx.send(desktop::Cmd::StartTray);
    }
    worker::start(host);
    app.run();
}
