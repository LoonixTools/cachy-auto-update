//! The window: a sidebar of panes, and the pane in a framed panel.

use crate::controls::{GUTTER, Paint};
use crate::ui::{Pane, Ui};
use cachy_auto_update::i18n::{tr, trf};
use cachy_auto_update::paths::VERSION;
use rune::{Cx, El, ScrollSpec, Tx, col, el, icon, row, text};

const NAV_ROW: f32 = 28.0;
const NAV_GAP: f32 = 4.0;

pub fn view(ui: &Ui, cx: &mut Cx<Ui>) -> El {
    let p = Paint::new(cx.system_dark(), cx.env().scale);
    let mut layer = Vec::new();
    let (actions, body) = match ui.pane {
        Pane::Overview => (crate::overview::actions(ui, p), crate::overview::pane(ui, p, &mut layer)),
        Pane::Schedule => (vec![], crate::settings::schedule(ui, p, &mut layer)),
        Pane::Updates => (vec![], crate::settings::updates(ui, p, &mut layer)),
        Pane::Notifications => (vec![], crate::settings::notifications(ui, p, &mut layer)),
        Pane::Cleanup => (vec![], crate::settings::cleanup(ui, p, &mut layer)),
        Pane::Log => (crate::log_pane::actions(ui, p), crate::log_pane::pane(ui, p)),
    };
    let body = if ui.pane == Pane::Log { body } else { scroll_body(p, body) };
    let panel = col()
        .key("panel")
        .role("main")
        .grow(1.0)
        .shrink()
        .min_h(0.0)
        .mt(8.0)
        .mr(8.0)
        .mb(8.0)
        .radius(p.k.radius.lg)
        .fill(p.k.c.bg_base)
        .border(p.hair, p.k.c.border_weak)
        .padding(p.hair)
        .clip_soft(0.0)
        .child(header(p, ui.pane.title(), actions))
        .children(ui.error.as_ref().map(|e| error_bar(p, e)))
        .child(body);
    let window = row()
        .key("window")
        .role("none")
        .w_full()
        .h_full()
        .fill(p.k.c.bg_deep)
        .child(sidebar(ui, p))
        .child(panel);
    rune_kit::root(window, layer)
}

/// The pane's title and its actions on the right.
fn header(p: Paint, title: &str, actions: Vec<El>) -> El {
    row()
        .key("pane-header")
        .h(56.0)
        .px(GUTTER)
        .gap(8.0)
        .center_y()
        .child(text(title).key("pane-title").role("heading").level(1).font(p.k.ty.body_medium).color(p.text()).shrink().truncate())
        .child(el().key("header-spacer").grow(1.0))
        .children(actions)
}

/// A problem the last click ran into, with a way to dismiss it.
fn error_bar(p: Paint, msg: &str) -> El {
    let c = &p.k.c;
    row()
        .key("error")
        .role("alert")
        .mx(GUTTER - 16.0)
        .mb(12.0)
        .px(14.0)
        .py(10.0)
        .gap(10.0)
        .center_y()
        .radius(p.k.radius.md)
        .fill(c.danger.bg)
        .child(icon(&crate::icons::WARNING, 16.0).key("icon").color(c.danger.fg).a11y_hidden())
        .child(text(msg).key("message").font(p.k.ty.ui).color(c.danger.fg).wrap().grow(1.0).shrink())
        .child(
            rune_kit::Button::new(tr("Dismiss"))
                .variant(rune_kit::ButtonVariant::Ghost)
                .on_press(|s: &mut Ui| s.error = None)
                .build(p.k)
                .key("dismiss"),
        )
}

/// A scrolling body whose edges fade while content hides under them.
fn scroll_body(p: Paint, content: El) -> El {
    let _ = p;
    col()
        .key("scroll")
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec { stick_bottom: false })
        .scroll_edge_fade(16.0)
        .child(col().key("content").w_full().pt(4.0).child(content))
}

fn sidebar(ui: &Ui, p: Paint) -> El {
    let mut nav = col().key("nav").role("list").gap(NAV_GAP).focus_group();
    for pane in Pane::ALL {
        nav = nav.child(nav_item(p, pane, ui.pane == pane, attention(ui, pane)));
    }
    col()
        .key("sidebar")
        .role("navigation")
        .w(220.0)
        .h_full()
        .pt(12.0)
        .pb(12.0)
        .pl(10.0)
        .pr(6.0)
        .child(row().key("brand").h(40.0).px(4.0).mb(8.0).gap(10.0).center_y().child(brand(ui, p)).child(
            text("Auto-Update").key("brand-name").font(p.k.ty.ui_medium).color(p.text()).truncate(),
        ))
        .child(nav.grow(1.0))
        .child(footer(p))
}

/// The app's logo; a tile with the update arrows until it is drawn.
fn brand(ui: &Ui, p: Paint) -> El {
    match ui.logo.as_ref().filter(|l| l.is_ready() && l.error().is_none()) {
        Some(logo) => el().key("logo").size(32.0, 32.0).pixels(logo.pixels()).a11y_hidden(),
        None => el().key("logo").size(26.0, 26.0).radius(7.0).center().fill(p.k.c.text_accent).a11y_hidden().child(
            icon(&crate::icons::REFRESH, 15.0).key("glyph").color(p.k.c.white),
        ),
    }
}

/// A dot on a pane that wants a look.
fn attention(ui: &Ui, pane: Pane) -> bool {
    match pane {
        Pane::Overview => {
            ui.status.result == cachy_auto_update::status::LastResult::Failed || ui.status.reboot || !ui.status.held.is_empty()
        }
        _ => false,
    }
}

fn nav_item(p: Paint, pane: Pane, on: bool, dot: bool) -> El {
    let c = &p.k.c;
    let mut item = row()
        .key(pane.key())
        .role("listitem")
        .label(pane.title())
        .focusable(true)
        .focus_selected(on)
        .focus_ring(2.0, -2.0, c.border_focus)
        .h(NAV_ROW)
        .px(6.0)
        .gap(8.0)
        .center_y()
        .radius(p.k.radius.sm)
        .hover(c.hover)
        .press(c.pressed)
        .on_press_tx(move |s: &mut Ui, tx: &mut Tx| {
            tx.instant();
            s.show_pane(pane);
        })
        .child(icon(pane.icon(), 16.0).key("icon").color(if on { c.icon_base } else { c.icon_muted }).a11y_hidden())
        .child(
            text(pane.title())
                .key("label")
                .font(p.k.ty.ui)
                .color(if on { c.text_base } else { c.text_muted })
                .grow(1.0)
                .truncate()
                .a11y_hidden(),
        )
        .children(dot.then(|| crate::controls::dot(c.warning.fg)));
    if on {
        item = item.fill(c.layer_02);
    }
    item
}

fn footer(p: Paint) -> El {
    col()
        .key("footer")
        .mt(8.0)
        .px(6.0)
        .gap(2.0)
        .child(text(trf("Version %s", &[&VERSION])).key("version").font(p.k.ty.small).color(p.faint()))
        .child(text("CachyOS Auto-Update").key("name").font(p.k.ty.small).color(p.faint()))
}
