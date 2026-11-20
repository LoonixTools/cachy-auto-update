//! The first pane: how things stand, the big button, and why an update would
//! wait right now.

use crate::controls::{Paint, ROW_PX, card, dot, icon_row, menu_width, page, section, section_note, select_row, toggle_row};
use crate::icons;
use crate::ui::{Pane, Ui};
use cachy_auto_update::config::{Kind, setting};
use cachy_auto_update::i18n::{tr, trf, ui_locale};
use cachy_auto_update::reboot::Then;
use cachy_auto_update::status::{self, LastResult, Next};
use rune::{El, col, el, icon, row, text};
use rune_kit::{Button, ButtonVariant, Menu, MenuItem, Placement, Progress, Spinner};

/// The menu next to Update now.
const UPDATE_MENU: &str = "update-menu";

pub fn actions(ui: &Ui, p: Paint) -> Vec<El> {
    vec![
        Button::new(tr("Check for updates"))
            .variant(ButtonVariant::Ghost)
            .disabled(ui.busy() || ui.checking)
            .on_press(|s: &mut Ui| s.check())
            .build(p.k)
            .key("check"),
    ]
}

pub fn pane(ui: &Ui, p: Paint, layer: &mut Vec<El>) -> El {
    let mut pg = page("overview").child(hero(ui, p, layer));
    if let Some(n) = notices(ui, p) {
        pg = pg.child(n);
    }
    pg.child(automatic(ui, p, layer)).child(conditions(ui, p)).child(last_update(ui, p))
}

/// The status card: what is happening, and the button that updates now.
fn hero(ui: &Ui, p: Paint, layer: &mut Vec<El>) -> El {
    let c = &p.k.c;
    let (glyph, tint) = if ui.busy() {
        (None, c.icon_accent)
    } else if ui.checking && ui.pending.is_none() {
        (None, c.icon_muted)
    } else if ui.pending.is_some_and(|n| n > 0) {
        (Some(&icons::DOWNLOAD), c.icon_accent)
    } else if ui.pending == Some(0) {
        (Some(&icons::CHECK_CIRCLE), c.success.fg)
    } else {
        (Some(&icons::REFRESH), c.icon_muted)
    };
    let badge = el().key("badge").size(48.0, 48.0).radius(p.k.radius.lg).center().fill(c.layer_02).a11y_hidden().child(match glyph {
        Some(g) => icon(g, 24.0).key("glyph").color(tint),
        None => Spinner::new().size(22.0).build(p.k).key("spinner"),
    });

    let loc = ui_locale();
    let mut sub = vec![trf("Last update: %s", &[&status::time_ago(&loc, ui.status.last_success)])];
    match ui.next {
        Next::At(ts) if ui.cfg.enabled => sub.push(trf("Next update: %s", &[&status::strftime(ts, "%a %x %H:%M")])),
        _ if !ui.cfg.enabled => sub.push(tr("Automatic updates are off.").into()),
        _ => {}
    }
    let mut copy = col()
        .key("copy")
        .grow(1.0)
        .shrink()
        .gap(4.0)
        .child(text(ui.headline()).key("headline").role("heading").level(2).font(p.k.ty.body_medium).color(p.text()).wrap())
        .child(text(sub.join("  ·  ")).key("when").font(p.k.ty.small).color(p.muted()).wrap());

    if ui.busy() {
        copy = copy.child(running(ui, p));
    }

    card(
        p,
        "hero",
        vec![row().key("hero-row").px(ROW_PX + 4.0).py(20.0).gap(16.0).center_y().child(badge).child(copy).child(update_button(ui, p, layer))],
    )
}

/// Update now, and an arrow at its side for Update and shut down or restart.
fn update_button(ui: &Ui, p: Paint, layer: &mut Vec<El>) -> El {
    let r = p.k.radius.md;
    let open = ui.open == Some(UPDATE_MENU);
    let now = Button::new(tr("Update now"))
        .variant(ButtonVariant::Contrast)
        .disabled(ui.busy())
        .on_press(|s: &mut Ui| s.update_now())
        .build(p.k)
        .key("update-now")
        .corners([r, 0.0, 0.0, r]);
    let chevron = p.k.glyph.chevron_down;
    let more = Button::new(tr("More update options"))
        .variant(ButtonVariant::Contrast)
        .disabled(ui.busy())
        .expanded(open)
        .label_with(move |_, ink| icon(chevron, 16.0).color(ink))
        .on_press(|s: &mut Ui| s.open = if s.open == Some(UPDATE_MENU) { None } else { Some(UPDATE_MENU) })
        .build(p.k)
        .key("update-more")
        .px(6.0)
        .corners([0.0, r, r, 0.0]);
    let (off, restart) = (tr("Update and shut down"), tr("Update and restart"));
    // Rune's menu does not grow for its labels, so it gets the longest one's
    // width, and the icon's.
    let longest = off.chars().count().max(restart.chars().count()) as f32;
    let menu = Menu::new(UPDATE_MENU)
        .trigger(more)
        .placement(Placement::BottomEnd)
        .min_width(menu_width(longest) + 22.0)
        .item(MenuItem::new(off).icon(&icons::POWER).on_select(|s: &mut Ui| s.update(Some(Then::PowerOff))))
        .item(MenuItem::new(restart).icon(&icons::RESTART).on_select(|s: &mut Ui| s.update(Some(Then::Reboot))))
        .open(open && !ui.busy())
        .on_close(|s: &mut Ui| s.open = None)
        .build(p.k, layer);
    row().key("update").gap(1.0).child(now).child(menu)
}

/// The bar of a running update, as the run reports it.
fn running(ui: &Ui, p: Paint) -> El {
    let prog = ui.progress.clone().unwrap_or_default();
    let label = if ui.starting && ui.progress.is_none() {
        tr("Starting the update...").to_string()
    } else if prog.paused {
        tr("Paused").to_string()
    } else if prog.label.is_empty() {
        tr("Installing updates").to_string()
    } else {
        tr(&prog.label).to_string()
    };
    let mut right = Vec::new();
    if prog.total > 0 && !prog.paused {
        right.push(trf("%d of %d", &[&prog.done.min(prog.total), &prog.total]));
    }
    right.push(format!("{}%", prog.percent));
    let detail = (!prog.detail.is_empty() && !prog.paused).then(|| {
        text(prog.detail.clone()).key("detail").font(crate::controls::DESCRIPTION).color(p.faint()).truncate()
    });
    col()
        .key("running")
        .mt(10.0)
        .gap(6.0)
        .child(
            row()
                .key("progress-line")
                .gap(8.0)
                .child(text(label).key("step").font(p.k.ty.small_medium).color(p.text()).grow(1.0).truncate())
                .child(text(right.join("  ·  ")).key("count").font(p.k.ty.small).color(p.muted())),
        )
        .child(Progress::new(Some(prog.percent as f32 / 100.0)).build(p.k).key("bar").label(tr("Update progress")))
        .children(detail)
        .child(
            text(ui.then.map_or(tr("Please leave the computer switched on until this is done."), crate::ui::then_text))
                .key("hint")
                .font(crate::controls::DESCRIPTION)
                .color(p.muted())
                .wrap(),
        )
}

/// Things that need a person, one row each.
fn notices(ui: &Ui, p: Paint) -> Option<El> {
    let c = &p.k.c;
    let st = &ui.status;
    let mut rows = Vec::new();
    let log_button = |key: &str| {
        Button::new(tr("Show log"))
            .variant(ButtonVariant::Outline)
            .on_press(|s: &mut Ui| s.show_pane(Pane::Log))
            .build(p.k)
            .key(key.to_string())
    };
    if !ui.busy() {
        match st.result {
            LastResult::Failed => rows.push(icon_row(
                p,
                "failed",
                &icons::WARNING,
                tr("The last update had a problem."),
                Some(tr("Everything else keeps working. The log says what went wrong.")),
                log_button("failed-log"),
            )),
            LastResult::Interrupted => rows.push(icon_row(
                p,
                "interrupted",
                &icons::WARNING,
                tr("The last run was stopped before it finished."),
                Some(tr("It is finished on the next run.")),
                el(),
            )),
            LastResult::Cancelled => rows.push(icon_row(
                p,
                "cancelled",
                &icons::PAUSE,
                tr("The last run was cancelled. It will run again later."),
                None,
                el(),
            )),
            _ => {}
        }
    }
    let planned = cachy_auto_update::reboot::will_happen(&ui.cfg, st.restart_planned);
    if st.reboot || planned {
        rows.push(icon_row(
            p,
            "reboot",
            &icons::RESTART,
            if st.reboot {
                tr("A restart is recommended to finish a kernel update.")
            } else {
                tr("A restart is planned to finish the update.")
            },
            Some(if planned {
                tr("It happens by itself once nobody is logged in.")
            } else {
                tr("Restart when it suits you. Nothing restarts by itself.")
            }),
            el(),
        ));
    }
    if !st.held.is_empty() {
        rows.push(icon_row(
            p,
            "held",
            &icons::LOCK,
            &trf("Held back: %s", &[&st.held.join(", ")]),
            Some(tr("These could not be updated and were skipped. They are tried again later.")),
            log_button("held-log"),
        ));
    }
    if rows.is_empty() {
        return None;
    }
    let _ = c;
    Some(section(p, "notices", tr("Needs a look"), rows))
}

fn automatic(ui: &Ui, p: Paint, layer: &mut Vec<El>) -> El {
    let loc = ui_locale();
    let choices = match setting("UpdateInterval").map(|s| s.kind) {
        Some(Kind::Interval(v)) => v.iter().map(|v| (v.to_string(), crate::controls::capitalize(&status::interval_text(&loc, v)))).collect(),
        _ => vec![],
    };
    section(
        p,
        "automatic",
        tr("Automatic updates"),
        vec![
            toggle_row(
                ui,
                p,
                "Enabled",
                tr("Update automatically"),
                Some(tr("Updates install in the background, with no password and no questions.")),
            ),
            select_row(ui, p, layer, "UpdateInterval", tr("How often to update"), None, choices),
        ],
    )
}

/// Why an update would wait right now.
fn conditions(ui: &Ui, p: Paint) -> El {
    let c = &p.k.c;
    let (good, wait) = (c.success.fg, c.warning.fg);
    let rows = match &ui.conditions {
        None => vec![icon_row(p, "loading", &icons::INFO, tr("Looking..."), None, el())],
        Some(cond) => {
            let power = if cond.ac {
                icon_row(p, "power", &icons::PLUG, tr("On AC power"), None, dot(good))
            } else {
                let title = match cond.battery {
                    Some(pct) => trf("On battery (%d%%)", &[&pct]),
                    None => tr("On battery").into(),
                };
                let low = cond.battery.is_some_and(|b| b < ui.cfg.min_battery) || ui.cfg.require_ac;
                let desc = if ui.cfg.require_ac {
                    tr("Updates wait until the computer is plugged in.").to_string()
                } else {
                    trf("Updates wait below %d%% battery.", &[&ui.cfg.min_battery])
                };
                icon_row(p, "power", &icons::BATTERY, &title, Some(&desc), dot(if low { wait } else { good }))
            };
            let game = match &cond.busy {
                Some(why) => icon_row(p, "game", &icons::GAMEPAD, &crate::controls::capitalize(why), Some(tr("Updates wait until it is closed.")), dot(wait)),
                None => icon_row(p, "game", &icons::GAMEPAD, tr("No game running"), None, dot(good)),
            };
            let pm = match &cond.package_manager {
                Some(why) => icon_row(
                    p,
                    "pacman",
                    &icons::PACKAGE,
                    &crate::controls::capitalize(why),
                    Some(tr("Updates wait so they do not get in the way.")),
                    dot(wait),
                ),
                None => icon_row(p, "pacman", &icons::PACKAGE, tr("Package system is free"), None, dot(good)),
            };
            vec![power, game, pm]
        }
    };
    section_note(p, "conditions", tr("Right now"), tr("An update waits while any of these is orange"), rows)
}


/// What the last successful update brought.
fn last_update(ui: &Ui, p: Paint) -> El {
    let st = &ui.status;
    let when = if st.last_success > 0 {
        status::strftime(st.last_success, "%a %x %H:%M")
    } else {
        tr("never").to_string()
    };
    let stat = |key: &str, n: u64, label: &str| {
        col()
            .key(key.to_string())
            .grow(1.0)
            .gap(2.0)
            .child(text(n.to_string()).key("n").font(p.k.ty.body_medium).color(p.text()))
            .child(text(label).key("label").font(p.k.ty.small).color(p.muted()).truncate())
    };
    let counts = row()
        .key("counts")
        .px(ROW_PX)
        .py(14.0)
        .gap(12.0)
        .child(stat("repo", st.counts[0], tr("System packages")))
        .child(stat("aur", st.counts[1], tr("AUR packages")))
        .child(stat("flatpak", st.counts[2], tr("Flatpaks")))
        .child(stat("appimage", st.counts[3], tr("AppImages")));
    section_note(p, "last", tr("Last update"), &when, vec![counts])
}
