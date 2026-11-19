//! The first pane: how things stand, the big button, and why an update would
//! wait right now.

use crate::controls::{DETAIL, Paint, button, capitalize, dot, icon_row, progress, section, section_note, select_row, spinner, surface, toggle_row};
use crate::icons;
use crate::ui::{Pane, Ui};
use crate::view::page;
use cachy_auto_update::config::{self, Kind};
use cachy_auto_update::i18n::{tr, trf, ui_locale};
use cachy_auto_update::status::{self, LastResult, Next};
use rune::tok::{TextStyle, radius, space};
use rune::{El, col, el, icon, row, text};

pub fn pane(ui: &Ui, p: Paint, layer: &mut Vec<El>) -> El {
    let check = button(p, "check", tr("Check for updates"), false, !(ui.busy() || ui.checking)).on_press(|s: &mut Ui| s.check());
    page(ui, p, Pane::Overview.title(), vec![check])
        .child(hero(ui, p))
        .children(notices(ui, p))
        .child(automatic(ui, p, layer))
        .child(conditions(ui, p))
        .child(last_update(ui, p))
}

/// The status card: what is happening, and the button that updates now.
fn hero(ui: &Ui, p: Paint) -> El {
    let glyph = if ui.busy() || (ui.checking && ui.pending.is_none()) {
        None
    } else if ui.pending.is_some_and(|n| n > 0) {
        Some((&icons::DOWNLOAD, p.accent))
    } else if ui.pending == Some(0) {
        Some((&icons::CHECK_CIRCLE, p.good()))
    } else {
        Some((&icons::REFRESH, p.muted()))
    };
    let badge = el().key("badge").size(48.0, 48.0).radius(radius::LG).center().fill(p.t.control_track).a11y_hidden().child(match glyph {
        Some((g, tint)) => icon(g, 24.0).key("glyph").color(tint),
        None => spinner(p.accent, 22.0),
    });

    let loc = ui_locale();
    let mut sub = vec![trf("Last update: %s", &[&status::time_ago(&loc, ui.status.last_success)])];
    match ui.next {
        Next::At(ts) if ui.cfg.enabled => sub.push(trf("Next update: %s", &[&status::strftime(ts, "%a %x %H:%M")])),
        _ if !ui.cfg.enabled => sub.push(tr("Automatic updates are off.").into()),
        _ => {}
    }
    let copy = col()
        .key("copy")
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .gap(3.0)
        .child(text(ui.headline()).key("headline").role("heading").level(2).font(TextStyle::new(15.0, 650.0)).color(p.text()).wrap())
        .child(text(sub.join("  ·  ")).key("when").font(DETAIL).color(p.muted()).wrap());
    let now = button(p, "update-now", tr("Update now"), true, !ui.busy()).on_press(|s: &mut Ui| s.update_now());
    surface(p)
        .key("hero")
        .padding(space::S3 + 4.0)
        .gap(space::S3 + 4.0)
        .child(row().key("hero-row").gap(14.0).center_y().child(badge).child(copy).child(now))
        .children(ui.busy().then(|| running(ui, p)))
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
    let detail = (!prog.detail.is_empty() && !prog.paused).then(|| text(prog.detail.clone()).key("detail").font(DETAIL).color(p.muted()).truncate());
    col()
        .key("running")
        .gap(6.0)
        .child(
            row()
                .key("progress-line")
                .gap(8.0)
                .child(text(label).key("step").font(rune::tok::type_::LABEL).color(p.text()).grow(1.0).truncate())
                .child(text(right.join("  ·  ")).key("count").font(DETAIL).color(p.muted())),
        )
        .child(progress(p, tr("Update progress"), prog.percent as f32 / 100.0))
        .children(detail)
        .child(text(tr("Please leave the computer switched on until this is done.")).key("hint").font(DETAIL).color(p.muted()).wrap())
}

/// Things that need a person, one row each.
fn notices(ui: &Ui, p: Paint) -> Option<El> {
    let st = &ui.status;
    let mut rows = Vec::new();
    let log_button = |key: &str| button(p, key, tr("Show log"), false, true).on_press(|s: &mut Ui| s.show_pane(Pane::Log));
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
            LastResult::Cancelled => {
                rows.push(icon_row(p, "cancelled", &icons::PAUSE, tr("The last run was cancelled. It will run again later."), None, el()))
            }
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
    (!rows.is_empty()).then(|| section(p, "notices", Some(tr("Needs a look")), rows))
}

fn automatic(ui: &Ui, p: Paint, layer: &mut Vec<El>) -> El {
    let loc = ui_locale();
    let choices = match config::setting("UpdateInterval").map(|s| s.kind) {
        Some(Kind::Interval(v)) => v.iter().map(|v| (v.to_string(), capitalize(&status::interval_text(&loc, v)))).collect(),
        _ => vec![],
    };
    section(
        p,
        "automatic",
        Some(tr("Automatic updates")),
        vec![
            toggle_row(ui, p, "Enabled", tr("Update automatically"), Some(tr("Updates install in the background, with no password and no questions."))),
            select_row(ui, p, layer, "UpdateInterval", tr("How often to update"), None, choices),
        ],
    )
}

/// Why an update would wait right now.
fn conditions(ui: &Ui, p: Paint) -> El {
    let (good, wait) = (p.good(), p.wait());
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
                Some(why) => icon_row(p, "game", &icons::GAMEPAD, &capitalize(why), Some(tr("Updates wait until it is closed.")), dot(wait)),
                None => icon_row(p, "game", &icons::GAMEPAD, tr("No game running"), None, dot(good)),
            };
            let pm = match &cond.package_manager {
                Some(why) => icon_row(p, "pacman", &icons::PACKAGE, &capitalize(why), Some(tr("Updates wait so they do not get in the way.")), dot(wait)),
                None => icon_row(p, "pacman", &icons::PACKAGE, tr("Package system is free"), None, dot(good)),
            };
            vec![power, game, pm]
        }
    };
    section_note(p, "conditions", Some(tr("Right now")), Some(tr("An update waits while any of these is orange")), rows)
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
            .child(text(n.to_string()).key("n").font(TextStyle::new(20.0, 700.0)).color(p.text()))
            .child(text(label).key("label").font(DETAIL).color(p.muted()).truncate())
    };
    let counts = row()
        .key("counts")
        .px(space::S3)
        .py(space::S3)
        .gap(12.0)
        .child(stat("repo", st.counts[0], tr("System packages")))
        .child(stat("aur", st.counts[1], tr("AUR packages")))
        .child(stat("flatpak", st.counts[2], tr("Flatpaks")))
        .child(stat("appimage", st.counts[3], tr("AppImages")));
    section_note(p, "last", Some(tr("Last update")), Some(&when), vec![counts])
}
