//! The log: what the last run did, or every run.

use crate::controls::{GUTTER, Paint};
use crate::ui::Ui;
use cachy_auto_update::i18n::tr;
use rune::{El, ScrollSpec, col, text};
use rune_kit::SegmentedControl;

pub fn actions(ui: &Ui, p: Paint) -> Vec<El> {
    vec![
        SegmentedControl::new(&[tr("Last run"), tr("All runs")], Some(usize::from(ui.log_all)))
            .label(tr("Which log"))
            .fit()
            .on_pick(|s: &mut Ui, i, _| s.set_log_all(i == 1))
            .build(p.k)
            .key("which-log"),
    ]
}

pub fn pane(ui: &Ui, p: Paint) -> El {
    let body = if ui.log.is_empty() {
        text(tr("No log yet.")).key("empty").font(p.k.ty.ui).color(p.muted())
    } else {
        text(ui.log.clone())
            .key("log-text")
            .font(p.k.ty.mono)
            .color(p.text())
            .wrap()
            .selectable(true)
            .selection_color(p.k.selection())
    };
    col()
        .key("log-frame")
        .grow(1.0)
        .min_h(0.0)
        .mx(GUTTER - 16.0)
        .mb(16.0)
        .radius(p.k.radius.md)
        .fill(p.k.c.layer_01)
        .border(p.hair, p.k.c.border_weak)
        .clip_soft(0.0)
        .child(
            col()
                .key("log-scroll")
                .grow(1.0)
                .min_h(0.0)
                .scroll(ScrollSpec { stick_bottom: true })
                .child(col().key("log-content").w_full().px(16.0).py(12.0).child(body)),
        )
}
