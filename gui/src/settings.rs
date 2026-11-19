//! The panes of settings. Every change is written at once.

use crate::controls::{Paint, page, section, select_row, setting_row, toggle_row};
use crate::ui::Ui;
use cachy_auto_update::config::{Kind, Reboot, setting};
use cachy_auto_update::i18n::{tr, trf, ui_locale};
use cachy_auto_update::status;
use rune::El;
use rune_kit::{Button, ButtonVariant, TextInput};

fn choices(key: &str, label: impl Fn(&str) -> String) -> Vec<(String, String)> {
    match setting(key).map(|s| s.kind) {
        Some(Kind::Choice(v) | Kind::Interval(v)) => v.iter().map(|v| (v.to_string(), label(v))).collect(),
        _ => vec![],
    }
}

pub fn schedule(ui: &Ui, p: Paint, layer: &mut Vec<El>) -> El {
    let loc = ui_locale();
    page("schedule")
        .child(section(
            p,
            "when",
            tr("When"),
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
            ],
        ))
        .child(section(
            p,
            "restart",
            tr("Restart"),
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
            tr("Wait while"),
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

pub fn updates(ui: &Ui, p: Paint, layer: &mut Vec<El>) -> El {
    let helper = |v: &str| if v == "auto" { tr("Automatic").to_string() } else { v.to_string() };
    let save = Button::new(tr("Save"))
        .variant(ButtonVariant::Outline)
        .disabled(ui.ignore.text.as_str().split_whitespace().collect::<Vec<_>>().join(" ") == ui.value("IgnorePkg"))
        .on_press(|s: &mut Ui| {
            let v = s.ignore.text.as_str().split_whitespace().collect::<Vec<_>>().join(" ");
            s.set("IgnorePkg", v);
        })
        .build(p.k)
        .key("save");
    let field = TextInput::new(&ui.ignore)
        .placeholder(tr("For example: linux nvidia"))
        .label(tr("Never update these packages"))
        .width(Some(260.0))
        .on_edit(|s: &mut Ui| &mut s.ignore)
        .on_submit(|s: &mut Ui, _| {
            let v = s.ignore.text.as_str().split_whitespace().collect::<Vec<_>>().join(" ");
            s.set("IgnorePkg", v);
        })
        .build(p.k)
        .key("ignore-field");
    page("updates")
        .child(section(
            p,
            "sources",
            tr("Sources"),
            vec![
                setting_row(
                    p,
                    "pacman",
                    tr("System packages"),
                    Some(tr("Always updated, with pacman.")),
                    rune::text(tr("Always")).font(p.k.ty.ui).color(p.muted()),
                ),
                toggle_row(ui, p, "UpdateAUR", tr("Update AUR packages"), Some(tr("Built with paru or yay. One that does not build is held back."))),
                toggle_row(ui, p, "UpdateFlatpak", tr("Update Flatpaks"), Some(tr("For the whole system and for each user."))),
                toggle_row(ui, p, "UpdateAppImages", tr("Update AppImages"), Some(tr("Through Gear Lever, while you are logged in."))),
            ],
        ))
        .child(section(
            p,
            "aur",
            tr("AUR"),
            vec![
                select_row(ui, p, layer, "AURHelper", tr("AUR helper"), None, choices("AURHelper", helper)),
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
            tr("Never update"),
            vec![setting_row(
                p,
                "IgnorePkg",
                tr("Never update these packages"),
                Some(tr("Package names, separated by spaces.")),
                rune::row().key("ignore-control").gap(8.0).center_y().child(field).child(save),
            )],
        ))
}

pub fn notifications(ui: &Ui, p: Paint) -> El {
    page("notifications")
        .child(section(
            p,
            "messages",
            tr("Messages"),
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
            tr("Desktop"),
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
    page("cleanup").child(section(
        p,
        "housekeeping",
        tr("After each update"),
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
