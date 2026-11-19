//! The panes of settings. Every change is written at once.

use crate::controls::{DETAIL, Paint, button, field, section, segmented, select_row, setting, toggle_row};
use crate::ui::{Pane, Ui};
use crate::view::page;
use cachy_auto_update::config::{self, Kind, Reboot};
use cachy_auto_update::i18n::{tr, trf, ui_locale};
use cachy_auto_update::status;
use rune::{El, text};

fn choices(key: &str, label: impl Fn(&str) -> String) -> Vec<(String, String)> {
    match config::setting(key).map(|s| s.kind) {
        Some(Kind::Choice(v) | Kind::Interval(v) | Kind::Hours(v)) => v.iter().map(|v| (v.to_string(), label(v))).collect(),
        _ => vec![],
    }
}

/// The offered hours, and the ones from the file if they are others.
fn hours_choices(ui: &Ui, loc: &str) -> Vec<(String, String)> {
    let mut c = choices("UpdateHours", |v| status::hours_text(loc, v));
    let current = ui.value("UpdateHours");
    if !c.iter().any(|(v, _)| *v == current) {
        c.insert(1, (current.clone(), status::hours_text(loc, &current)));
    }
    c
}

pub fn schedule(ui: &Ui, p: Paint, layer: &mut Vec<El>) -> El {
    let loc = ui_locale();
    page(ui, p, Pane::Schedule.title(), vec![])
        .child(section(
            p,
            "when",
            Some(tr("When")),
            vec![
                toggle_row(
                    ui,
                    p,
                    "Enabled",
                    tr("Update automatically"),
                    Some(tr("A check runs every hour and updates once the time below has passed.")),
                ),
                select_row(
                    ui,
                    p,
                    layer,
                    "UpdateInterval",
                    tr("How often to update"),
                    None,
                    choices("UpdateInterval", |v| crate::controls::capitalize(&status::interval_text(&loc, v))),
                ),
                select_row(
                    ui,
                    p,
                    layer,
                    "UpdateHours",
                    tr("When to update"),
                    Some(tr("Updates only start in these hours. Update now works at any time.")),
                    hours_choices(ui, &loc),
                ),
            ],
        ))
        .child(section(
            p,
            "restart",
            Some(tr("Restart")),
            vec![select_row(
                ui,
                p,
                layer,
                "AutoReboot",
                tr("Restart automatically"),
                Some(tr("Only while nobody is logged in, and never after a failed update.")),
                choices("AutoReboot", |v| status::reboot_text(&loc, Reboot::parse(v)).to_string()),
            )],
        ))
        .child(section(
            p,
            "wait",
            Some(tr("Wait while")),
            vec![
                toggle_row(
                    ui,
                    p,
                    "SkipWhenGaming",
                    tr("Postpone while a game is running"),
                    Some(tr("Also while a video plays full screen. It tries again an hour later.")),
                ),
                toggle_row(ui, p, "RequireAC", tr("Only update on mains power"), None),
                select_row(
                    ui,
                    p,
                    layer,
                    "MinBatteryPercent",
                    tr("Minimum battery level"),
                    Some(tr("Below this, a laptop on battery waits.")),
                    choices("MinBatteryPercent", |v| if v == "0" { tr("No minimum").into() } else { format!("{v} %") }),
                ),
            ],
        ))
}

/// The packages typed into the field, the way the file keeps them.
fn typed(s: &Ui) -> String {
    s.ignore.text.as_str().split_whitespace().collect::<Vec<_>>().join(" ")
}

fn save_ignore(s: &mut Ui) {
    let v = typed(s);
    s.set("IgnorePkg", v);
}

pub fn updates(ui: &Ui, p: Paint, layer: &mut Vec<El>) -> El {
    let _ = layer;
    let changed = typed(ui) != ui.value("IgnorePkg");
    let save = button(p, "save", tr("Save"), changed, changed).on_press(save_ignore);
    let field = field(p, "ignore-field", &ui.ignore, tr("For example: linux nvidia"), 220.0, |s| &mut s.ignore, save_ignore);
    // The AUR helper: a segment each, as few as there are.
    let helpers: Vec<&str> = match config::setting("AURHelper").map(|s| s.kind) {
        Some(Kind::Choice(v)) => v.to_vec(),
        _ => vec![],
    };
    let names: Vec<&str> = helpers.iter().map(|v| if *v == "auto" { tr("Automatic") } else { v }).collect();
    let current = ui.value("AURHelper");
    let chosen = helpers.iter().position(|v| *v == current).unwrap_or(0);
    let helper = segmented(p, "aur-helper", tr("AUR helper"), &names, chosen, 260.0, move |s, i| {
        if let Some(v) = helpers.get(i) {
            s.set("AURHelper", v.to_string());
        }
    });
    page(ui, p, Pane::Updates.title(), vec![])
        .child(section(
            p,
            "sources",
            Some(tr("Sources")),
            vec![
                setting(p, "pacman", tr("System packages"), Some(tr("Always updated, with pacman.")), text(tr("Always")).font(DETAIL).color(p.muted())),
                toggle_row(ui, p, "UpdateAUR", tr("Update AUR packages"), Some(tr("Built with paru or yay. One that does not build is held back."))),
                toggle_row(ui, p, "UpdateFlatpak", tr("Update Flatpaks"), Some(tr("For the whole system and for each user."))),
                toggle_row(ui, p, "UpdateAppImages", tr("Update AppImages"), Some(tr("Through Gear Lever, while you are logged in."))),
            ],
        ))
        .child(section(
            p,
            "aur",
            Some(tr("AUR")),
            vec![
                setting(p, "AURHelper", tr("AUR helper"), None, helper),
                toggle_row(
                    ui,
                    p,
                    "UpdateDevel",
                    tr("Also rebuild -git packages"),
                    Some(tr("They rebuild often and break more often than released versions.")),
                ),
                toggle_row(
                    ui,
                    p,
                    "AutoResolveConflicts",
                    tr("Resolve package conflicts automatically"),
                    Some(tr("When a new package replaces an old one, say yes instead of stopping.")),
                ),
            ],
        ))
        .child(section(
            p,
            "ignore",
            Some(tr("Never update")),
            vec![setting(
                p,
                "IgnorePkg",
                tr("Never update these packages"),
                Some(tr("Package names, separated by spaces.")),
                rune::row().key("ignore-control").gap(8.0).center_y().child(field).child(save),
            )],
        ))
}

pub fn notifications(ui: &Ui, p: Paint) -> El {
    page(ui, p, Pane::Notifications.title(), vec![])
        .child(section(
            p,
            "messages",
            Some(tr("Messages")),
            vec![
                toggle_row(ui, p, "Notifications", tr("Show notifications"), Some(tr("Turns off every message below."))),
                toggle_row(
                    ui,
                    p,
                    "NotifyOnStart",
                    tr("Notify when an update starts"),
                    Some(tr("With a progress bar that has Pause and Cancel.")),
                ),
                toggle_row(
                    ui,
                    p,
                    "MinimizeProgress",
                    tr("Start the progress bar minimized"),
                    Some(tr("It waits in the notification area instead of popping up.")),
                ),
                toggle_row(ui, p, "NotifyOnSuccess", tr("Notify after a successful update"), None),
                toggle_row(ui, p, "NotifyOnError", tr("Notify when something goes wrong"), None),
            ],
        ))
        .child(section(
            p,
            "desktop",
            Some(tr("Desktop")),
            vec![
                toggle_row(
                    ui,
                    p,
                    "TrayIcon",
                    tr("Show an icon in the system tray"),
                    Some(tr("It shows when updates are waiting. A click opens this window.")),
                ),
                toggle_row(
                    ui,
                    p,
                    "CachyUpdate",
                    tr("Also run cachy-update"),
                    Some(tr("Its own tray icon and its daily check. Not needed while updates install by themselves.")),
                ),
                toggle_row(
                    ui,
                    p,
                    "RebootNotification",
                    tr("Show the CachyOS restart notice"),
                    Some(tr("It pops up in the middle of an update, so it is off.")),
                ),
            ],
        ))
}

pub fn cleanup(ui: &Ui, p: Paint, layer: &mut Vec<El>) -> El {
    let keep = |v: &str| match v {
        "0" => tr("None").to_string(),
        "1" => tr("1 version").to_string(),
        n => trf("%s versions", &[&n]),
    };
    page(ui, p, Pane::Cleanup.title(), vec![]).child(section(
        p,
        "housekeeping",
        Some(tr("After each update")),
        vec![
            toggle_row(
                ui,
                p,
                "CleanCache",
                tr("Trim the package cache"),
                Some(tr("Removes old package files. Installed software is never touched.")),
            ),
            select_row(
                ui,
                p,
                layer,
                "KeepOldPackages",
                tr("Cached versions to keep"),
                Some(tr("More versions leave room to go back to an older one.")),
                choices("KeepOldPackages", keep),
            ),
            toggle_row(
                ui,
                p,
                "CleanAURCache",
                tr("Clean the AUR build caches"),
                Some(tr("Removes build leftovers of paru, yay and Shelly. The recipes stay.")),
            ),
            toggle_row(
                ui,
                p,
                "RemoveOrphans",
                tr("Remove packages nothing needs any more"),
                Some(tr("This removes installed software, so it is off by default.")),
            ),
        ],
    ))
}
