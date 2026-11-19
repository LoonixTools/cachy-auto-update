//! The session bus: one copy of the app per session, and the tray icon.
//!
//! The app holds a well-known name. A second start (the menu entry, the
//! autostart, `cachy-auto-update` in a terminal) finds it taken, asks the
//! first copy to show its window, and leaves.
//!
//! The icon is a StatusNotifierItem with a com.canonical.dbusmenu menu, on a
//! connection of its own: closing that connection takes the icon off the
//! tray at once, which is how it goes when TrayIcon is switched off.

use crate::ui::{Pane, Ui};
use cachy_auto_update::i18n::tr;
use cachy_auto_update::paths::PRETTY;
use futures_util::StreamExt;
use rune::AppHandle;
use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use zbus::object_server::SignalEmitter;
use zbus::{Connection, OwnedObjectPath, OwnedVariant, Proxy, Variant};

pub const APP_NAME: &str = "io.github.loonixtools.CachyAutoUpdate";
const APP_PATH: &str = "/io/github/loonixtools/CachyAutoUpdate";
const ITEM_PATH: &str = "/StatusNotifierItem";
const MENU_PATH: &str = "/MenuBar";
const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";
const TIMEOUT: Duration = Duration::from_secs(5);

/// What the tray icon shows.
#[derive(Clone, Debug, PartialEq)]
pub struct Look {
    pub icon: &'static str,
    pub body: String,
    pub busy: bool,
    pub checking: bool,
}

pub enum Cmd {
    Look(Look),
    StartTray,
    StopTray,
}

/// The app, once it exists. Clicks that come before are dropped.
pub type Handle = Arc<OnceLock<AppHandle<Ui>>>;

fn with_app(h: &Handle, f: impl FnOnce(&AppHandle<Ui>)) {
    if let Some(app) = h.get() {
        f(app);
    }
}

/// Shows the window on a pane, with focus when an activation token came.
fn show(h: &Handle, pane: Pane, token: Option<String>) {
    with_app(h, |app| {
        app.update(move |s, _| s.show_pane(pane));
        match token {
            Some(t) if !t.is_empty() => app.activate(t),
            _ => app.open(),
        }
    });
}

struct AppIface {
    handle: Handle,
}

#[zbus::interface(name = "io.github.loonixtools.CachyAutoUpdate")]
impl AppIface {
    /// Shows the window on a pane ("overview", "log", ...).
    fn show(&self, pane: String, token: String) {
        show(&self.handle, Pane::from_name(&pane), Some(token));
    }
}

const UPDATE_NOW: i32 = 1;
const CHECK: i32 = 2;
const SEPARATOR: i32 = 3;
const SHOW_LOG: i32 = 4;
const SETTINGS: i32 = 5;

struct Model {
    look: Look,
    revision: u32,
    token: Option<String>,
}
type Shared = Arc<Mutex<Model>>;

/// Icon pictures as width, height and ARGB bytes; the icon name is used instead.
type Pixmaps = Vec<(i32, i32, Vec<u8>)>;

/// One dbusmenu node, `(ia{sv}av)`.
type Layout = (i32, HashMap<String, OwnedVariant>, Vec<OwnedVariant>);

fn items(look: &Look) -> Vec<(i32, Vec<(&'static str, OwnedVariant)>)> {
    let s = |v: &str| OwnedVariant::from(zbus::Str::from(v.to_string()));
    let action = |label: &str, icon: &str, enabled: bool| {
        let mut p = vec![("label", s(label)), ("icon-name", s(icon))];
        if !enabled {
            p.push(("enabled", OwnedVariant::from(false)));
        }
        p
    };
    vec![
        (UPDATE_NOW, action(tr("Update now"), "update-low", !look.busy)),
        (CHECK, action(tr("Check for updates"), "view-refresh", !(look.busy || look.checking))),
        (SEPARATOR, vec![("type", s("separator"))]),
        (SHOW_LOG, action(tr("Show log"), "utilities-terminal", true)),
        (SETTINGS, action(tr("Settings"), "configure", true)),
    ]
}

fn properties(look: &Look, id: i32) -> HashMap<String, OwnedVariant> {
    if id == 0 {
        return HashMap::from([("children-display".to_string(), OwnedVariant::from(zbus::Str::from_static("submenu")))]);
    }
    items(look).into_iter().find(|(i, _)| *i == id).map(|(_, p)| p.into_iter().map(|(k, v)| (k.to_string(), v)).collect()).unwrap_or_default()
}

fn layout(look: &Look) -> Layout {
    let children = items(look)
        .into_iter()
        .filter_map(|(id, _)| OwnedVariant::try_from(Variant::from((id, properties(look, id), Vec::<OwnedVariant>::new()))).ok())
        .collect();
    (0, properties(look, 0), children)
}

struct Item {
    model: Shared,
    handle: Handle,
}

#[zbus::interface(name = "org.kde.StatusNotifierItem")]
impl Item {
    /// A click installs the updates. While an update runs, it shows what it
    /// does instead of starting a second one.
    fn activate(&self, _x: i32, _y: i32) {
        let (token, busy) = {
            let mut m = self.model.lock().unwrap();
            (m.token.take(), m.look.busy)
        };
        if busy {
            show(&self.handle, Pane::Log, token);
        } else {
            with_app(&self.handle, |a| a.update(|s, _| s.update_now()));
        }
    }
    /// A middle click shows what the update does.
    fn secondary_activate(&self, _x: i32, _y: i32) {
        let token = self.model.lock().unwrap().token.take();
        show(&self.handle, Pane::Log, token);
    }
    fn context_menu(&self, _x: i32, _y: i32) {}
    fn scroll(&self, _delta: i32, _orientation: &str) {}
    fn provide_xdg_activation_token(&self, token: String) {
        self.model.lock().unwrap().token = Some(token);
    }
    #[zbus(property)]
    fn category(&self) -> &str {
        "SystemServices"
    }
    #[zbus(property)]
    fn id(&self) -> &str {
        "cachy-auto-update"
    }
    #[zbus(property)]
    fn title(&self) -> &str {
        PRETTY
    }
    #[zbus(property)]
    fn status(&self) -> &str {
        "Active"
    }
    #[zbus(property)]
    fn window_id(&self) -> i32 {
        0
    }
    #[zbus(property)]
    fn icon_name(&self) -> String {
        self.model.lock().unwrap().look.icon.to_string()
    }
    #[zbus(property)]
    fn icon_pixmap(&self) -> Pixmaps {
        Vec::new()
    }
    #[zbus(property)]
    fn overlay_icon_name(&self) -> &str {
        ""
    }
    #[zbus(property)]
    fn attention_icon_name(&self) -> &str {
        ""
    }
    #[zbus(property)]
    fn tool_tip(&self) -> (String, Pixmaps, String, String) {
        let m = self.model.lock().unwrap();
        let body = format!("{}\n\n{}", m.look.body, if m.look.busy { tr("Click to see what it is doing") } else { tr("Click to update now") });
        (m.look.icon.to_string(), Vec::new(), PRETTY.to_string(), body)
    }
    #[zbus(property)]
    fn item_is_menu(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn menu(&self) -> OwnedObjectPath {
        OwnedObjectPath::try_from(MENU_PATH).expect("an object path")
    }
    #[zbus(signal)]
    async fn new_icon(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn new_tool_tip(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

struct Menu {
    model: Shared,
    handle: Handle,
}

impl Menu {
    fn clicked(&self, id: i32, event: &str) {
        if event != "clicked" {
            return;
        }
        match id {
            UPDATE_NOW => with_app(&self.handle, |a| a.update(|s, _| s.update_now())),
            CHECK => with_app(&self.handle, |a| a.update(|s, _| s.check())),
            SHOW_LOG => show(&self.handle, Pane::Log, None),
            SETTINGS => show(&self.handle, Pane::Overview, None),
            _ => {}
        }
    }
}

#[zbus::interface(name = "com.canonical.dbusmenu")]
impl Menu {
    fn get_layout(&self, _parent: i32, _depth: i32, _names: Vec<String>) -> (u32, Layout) {
        let m = self.model.lock().unwrap();
        (m.revision, layout(&m.look))
    }
    fn get_group_properties(&self, ids: Vec<i32>, _names: Vec<String>) -> Vec<(i32, HashMap<String, OwnedVariant>)> {
        let m = self.model.lock().unwrap();
        let ids = if ids.is_empty() { items(&m.look).into_iter().map(|(i, _)| i).collect() } else { ids };
        ids.into_iter().map(|id| (id, properties(&m.look, id))).collect()
    }
    fn get_property(&self, id: i32, name: String) -> zbus::fdo::Result<OwnedVariant> {
        properties(&self.model.lock().unwrap().look, id)
            .remove(&name)
            .ok_or_else(|| zbus::fdo::Error::InvalidArgs("no such property".into()))
    }
    fn event(&self, id: i32, event_id: String, _data: OwnedVariant, _timestamp: u32) {
        self.clicked(id, &event_id);
    }
    fn event_group(&self, events: Vec<(i32, String, OwnedVariant, u32)>) -> Vec<i32> {
        for (id, event, _, _) in events {
            self.clicked(id, &event);
        }
        Vec::new()
    }
    fn about_to_show(&self, _id: i32) -> bool {
        false
    }
    fn about_to_show_group(&self, _ids: Vec<i32>) -> (Vec<i32>, Vec<i32>) {
        (Vec::new(), Vec::new())
    }
    #[zbus(property)]
    fn version(&self) -> u32 {
        3
    }
    #[zbus(property)]
    fn text_direction(&self) -> &str {
        "ltr"
    }
    #[zbus(property)]
    fn status(&self) -> &str {
        "normal"
    }
    #[zbus(property)]
    fn icon_theme_path(&self) -> Vec<String> {
        Vec::new()
    }
    #[zbus(signal)]
    async fn layout_updated(emitter: &SignalEmitter<'_>, revision: u32, parent: i32) -> zbus::Result<()>;
}

/// Asks the watcher to show the icon. No watcher means no tray yet.
async fn register(conn: &Connection) {
    let Some(name) = conn.unique_name().map(|n| n.to_string()) else { return };
    let call = async {
        let watcher = Proxy::new(conn, WATCHER, WATCHER_PATH, WATCHER).await?;
        watcher.call::<_, _, ()>("RegisterStatusNotifierItem", &(name.as_str(),)).await
    };
    let _ = tokio::time::timeout(TIMEOUT, call).await;
}

async fn start_tray(model: &Shared, handle: &Handle) -> Option<(Connection, zbus::proxy::SignalStream<'static>)> {
    let setup = async {
        let conn = zbus::connection::Builder::session()
            .serve_at(ITEM_PATH, Item { model: model.clone(), handle: handle.clone() })
            .serve_at(MENU_PATH, Menu { model: model.clone(), handle: handle.clone() })
            .build()
            .await?;
        let bus = Proxy::new(&conn, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus").await?;
        let owners = bus.receive_signal_with_args("NameOwnerChanged", &[(0, WATCHER)]).await?;
        Ok::<_, zbus::Error>((conn, owners))
    };
    let (conn, owners) = tokio::time::timeout(TIMEOUT, setup).await.ok()?.ok()?;
    register(&conn).await;
    Some((conn, owners))
}

/// What came of claiming the app's name.
pub enum Claim {
    /// This is the only copy. Commands for the bus thread go here.
    First(Sender<Cmd>),
    /// Another copy runs and was asked to show itself.
    Other,
    /// No session bus: run without the icon and without the guard.
    NoBus,
}

/// Starts the bus thread. It claims the name, and serves the icon once a
/// StartTray arrives.
pub fn start(handle: Handle, pane: Pane, show_other: bool) -> Claim {
    let (tx, rx) = std::sync::mpsc::channel::<Cmd>();
    let (ready, result) = std::sync::mpsc::sync_channel::<Claim>(1);
    let cmd_tx = tx.clone();
    std::thread::Builder::new()
        .name("cau-desktop".into())
        .spawn(move || {
            let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
                let _ = ready.send(Claim::NoBus);
                return;
            };
            rt.block_on(async move {
                let Ok(Ok(conn)) = tokio::time::timeout(TIMEOUT, Connection::session()).await else {
                    let _ = ready.send(Claim::NoBus);
                    return;
                };
                let flags = zbus::fdo::RequestNameFlags::DO_NOT_QUEUE;
                let got = conn.request_name_with_flags(APP_NAME, flags).await;
                if !matches!(got, Ok(zbus::fdo::RequestNameReply::PrimaryOwner)) {
                    if show_other {
                        let token = std::env::var("XDG_ACTIVATION_TOKEN").unwrap_or_default();
                        let name = format!("{pane:?}").to_lowercase();
                        let _ = conn.call_method(Some(APP_NAME), APP_PATH, Some(APP_NAME), "Show", &(name, token)).await;
                    }
                    let _ = ready.send(Claim::Other);
                    return;
                }
                let _ = conn.object_server().at(APP_PATH, AppIface { handle: handle.clone() }).await;
                let _ = ready.send(Claim::First(cmd_tx));
                serve(rx, handle).await;
                drop(conn);
            });
        })
        .ok();
    result.recv().unwrap_or(Claim::NoBus)
}

/// Commands arrive on a std channel; a blocking thread hands them over.
async fn serve(rx: std::sync::mpsc::Receiver<Cmd>, handle: Handle) {
    let (atx, mut arx) = tokio::sync::mpsc::unbounded_channel();
    std::thread::spawn(move || {
        while let Ok(c) = rx.recv() {
            if atx.send(c).is_err() {
                break;
            }
        }
    });
    let model: Shared = Arc::new(Mutex::new(Model {
        look: Look { icon: "update-none", body: String::new(), busy: false, checking: false },
        revision: 1,
        token: None,
    }));
    let mut tray: Option<(Connection, zbus::proxy::SignalStream<'static>)> = None;
    let mut watch_done = false;
    loop {
        let owner = async {
            match tray.as_mut() {
                Some((_, owners)) if !watch_done => owners.next().await,
                _ => std::future::pending().await,
            }
        };
        tokio::select! {
            cmd = arx.recv() => {
                let Some(cmd) = cmd else { break };
                match cmd {
                    Cmd::StartTray => {
                        if tray.is_none() {
                            tray = start_tray(&model, &handle).await;
                            watch_done = false;
                        }
                    }
                    Cmd::StopTray => {
                        if let Some((conn, _)) = tray.take() {
                            let _ = conn.close().await;
                        }
                    }
                    Cmd::Look(look) => {
                        let (icon, menu) = {
                            let mut m = model.lock().unwrap();
                            let icon = m.look.icon != look.icon;
                            let menu = (m.look.busy, m.look.checking) != (look.busy, look.checking);
                            m.look = look;
                            if menu {
                                m.revision += 1;
                            }
                            (icon, menu.then_some(m.revision))
                        };
                        if let Some((conn, _)) = &tray {
                            if let Ok(e) = SignalEmitter::new(conn, ITEM_PATH) {
                                if icon {
                                    let _ = Item::new_icon(&e).await;
                                }
                                let _ = Item::new_tool_tip(&e).await;
                            }
                            if let (Some(rev), Ok(e)) = (menu, SignalEmitter::new(conn, MENU_PATH)) {
                                let _ = Menu::layout_updated(&e, rev, 0).await;
                            }
                        }
                    }
                }
            }
            changed = owner => {
                // The stream ended: stop asking it, or this loop would spin.
                if changed.is_none() {
                    watch_done = true;
                }
                // A tray that starts later or restarts gets the icon again.
                let new = changed.and_then(|c| c.body().deserialize::<(String, String, String)>().ok()).map(|(_, _, new)| new);
                if new.is_some_and(|n| !n.is_empty())
                    && let Some((conn, _)) = &tray
                {
                    register(conn).await;
                }
            }
        }
    }
}
