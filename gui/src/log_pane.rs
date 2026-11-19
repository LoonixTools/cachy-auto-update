//! The log: what the last run did, or every run.

use crate::controls::{MONO, Paint, segmented, surface};
use crate::ui::{Pane, Ui};
use crate::view::page;
use cachy_auto_update::i18n::tr;
use rune::tok::{space, type_};
use rune::{El, ScrollSpec, col, text};

pub fn pane(ui: &Ui, p: Paint) -> El {
    let which = segmented(p, "which-log", tr("Which log"), &[tr("Last run"), tr("All runs")], usize::from(ui.log_all), 200.0, |s, i| {
        s.set_log_all(i == 1)
    });
    let body = if ui.log.is_empty() {
        text(tr("No log yet.")).key("empty").font(type_::BODY).color(p.muted())
    } else {
        text(ui.log.clone()).key("log-text").font(MONO).color(p.text()).wrap().selectable(true).selection_color(p.accent.alpha(0.35))
    };
    // The page fills the window, and the card the rest of it.
    page(ui, p, Pane::Log.title(), vec![which]).h_full().min_h(0.0).child(
        surface(p).key("log-frame").grow(1.0).min_h(0.0).clip_soft(0.0).child(
            col()
                .key("log-scroll")
                .grow(1.0)
                .min_h(0.0)
                .scroll(ScrollSpec { stick_bottom: true })
                .child(col().key("log-content").w_full().px(space::S3 + 4.0).py(space::S3).child(body)),
        ),
    )
}
