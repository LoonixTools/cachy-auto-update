//! The window, laid out like Waypointer's settings: a sidebar with the app's
//! name and its pages, a hairline, then the page with its big title.

use crate::controls::{DETAIL, Paint, button, title_row};
use crate::ui::{Pane, Ui};
use cachy_auto_update::i18n::{tr, trf};
use cachy_auto_update::paths::VERSION;
use cachy_auto_update::status::LastResult;
use rune::tok::{TextStyle, motion, radius, space};
use rune::{Color, Cx, El, ScrollSpec, Spring, SpringPair, Transition, Tx, col, el, icon, row, text};

/// Sidebar rows, as in Waypointer: 30 points tall with a 4 point gap.
const NAV_ROW: f32 = 30.0;
const NAV_GAP: f32 = 4.0;
const NAV_WIDTH: f32 = 220.0;

pub fn view(ui: &Ui, cx: &mut Cx<Ui>) -> El {
    let p = Paint::new(cx.system_dark(), cx.env().scale);
    let mut layer = Vec::new();
    let page = match ui.pane {
        Pane::Overview => crate::overview::pane(ui, p, &mut layer),
        Pane::Schedule => crate::settings::schedule(ui, p, &mut layer),
        Pane::Updates => crate::settings::updates(ui, p, &mut layer),
        Pane::Notifications => crate::settings::notifications(ui, p),
        Pane::Cleanup => crate::settings::cleanup(ui, p, &mut layer),
        Pane::Log => crate::log_pane::pane(ui, p),
    };
    // The old page goes at once and the new one comes up quickly, so the two
    // never show over each other.
    let page = page.key(format!("page-{}", ui.pane.key())).transition(
        Transition::asymmetric(Transition::offset(0.0, 6.0).combined(Transition::opacity()), Transition::none())
            .springs(SpringPair::new(Spring::new(0.28, 0.0), Spring::NONE)),
    );
    let content = if ui.pane == Pane::Log {
        // The log scrolls on its own and keeps to its end.
        col().key("content").role("main").grow(1.0).h_full().min_h(0.0).child(page)
    } else {
        col().key("content").role("main").grow(1.0).h_full().min_h(0.0).scroll(ScrollSpec { stick_bottom: false }).child(page)
    };
    let mut root = row()
        .key("window")
        .w_full()
        .h_full()
        .fill(p.t.bg)
        .child(sidebar(ui, p))
        .child(el().key("divider").w(p.hair).h_full().fill(p.t.separator))
        .child(content);
    // An open menu sits on a scrim that closes it.
    if !layer.is_empty() {
        root = root.child(el().key("scrim").absolute(0.0, 0.0).w_full().h_full().on_press(|s: &mut Ui| s.open = None));
    }
    root.children(layer)
}

/// A page: its big title with `actions` at the end of the line, a problem
/// the last click ran into, then its sections.
pub fn page(ui: &Ui, p: Paint, title: &str, actions: Vec<El>) -> El {
    col()
        .px(space::S6 + 4.0)
        .pt(space::S6)
        .pb(space::S6 + 8.0)
        .gap(space::S6)
        .child(title_row(p, title, actions))
        .children(ui.error.as_ref().map(|e| error_bar(p, e)))
}

fn error_bar(p: Paint, msg: &str) -> El {
    row()
        .key("error")
        .role("alert")
        .px(space::S3)
        .py(10.0)
        .gap(10.0)
        .center_y()
        .radius(radius::LG)
        .fill(p.danger().alpha(0.14))
        .border(p.hair, p.danger().alpha(0.4))
        .child(icon(&crate::icons::WARNING, 16.0).key("icon").color(p.danger()).a11y_hidden())
        .child(text(msg).key("message").font(rune::tok::type_::BODY).color(p.text()).wrap().grow(1.0).shrink())
        .child(button(p, "dismiss", tr("Dismiss"), false, true).on_press(|s: &mut Ui| s.error = None))
}

/// A dot on a pane that wants a look.
fn attention(ui: &Ui, pane: Pane) -> bool {
    pane == Pane::Overview && (ui.status.result == LastResult::Failed || ui.status.reboot || !ui.status.held.is_empty())
}

fn sidebar(ui: &Ui, p: Paint) -> El {
    let index = Pane::ALL.iter().position(|&x| x == ui.pane).unwrap_or(0);
    let inner = NAV_WIDTH - 20.0;
    // One highlight under the rows that springs to the chosen one. The rows
    // themselves never change shape, so nothing in them jumps.
    let mut nav = col().key("nav").role("list").gap(NAV_GAP).focus_group().child(
        el().key("nav-highlight")
            .size(inner, NAV_ROW)
            .absolute(0.0, index as f32 * (NAV_ROW + NAV_GAP))
            .radius(7.0)
            .fill(if p.dark { Color::WHITE.alpha(0.075) } else { Color::BLACK.alpha(0.06) })
            .motion(motion::SNAPPY)
            .a11y_hidden(),
    );
    for pane in Pane::ALL {
        let on = pane == ui.pane;
        nav = nav.child(
            row()
                .key(pane.key())
                .role("tab")
                .label(pane.title())
                .checked(on)
                .focusable(true)
                .w(inner)
                .h(NAV_ROW)
                .px(8.0)
                .gap(10.0)
                .center_y()
                .radius(7.0)
                .hover(if on { Color::TRANSPARENT } else { p.t.hover })
                .press(p.t.press)
                .on_press_tx(move |s: &mut Ui, tx: &mut Tx| {
                    tx.instant();
                    s.show_pane(pane);
                })
                .child(icon(pane.icon(), 16.0).key("icon").color(if on { p.accent } else { p.muted() }).a11y_hidden())
                .child(
                    text(pane.title())
                        .key("label")
                        .font(TextStyle::new(13.0, if on { 600.0 } else { 500.0 }))
                        .color(if on { p.text() } else { p.muted() })
                        .grow(1.0)
                        .truncate()
                        .a11y_hidden(),
                )
                .children(attention(ui, pane).then(|| crate::controls::dot(p.wait()))),
        );
    }
    col()
        .key("sidebar")
        .role("navigation")
        .w(NAV_WIDTH)
        .h_full()
        .pt(12.0)
        .pb(12.0)
        .px(10.0)
        .fill(if p.dark { Color::BLACK.alpha(0.12) } else { Color::BLACK.alpha(0.025) })
        .child(
            row()
                .key("brand")
                .h(44.0)
                .gap(10.0)
                .center_y()
                .px(4.0)
                .mb(10.0)
                .child(brand(ui, p))
                .child(text("Auto-Update").key("brand-name").font(TextStyle::new(14.0, 650.0)).color(p.text()).truncate()),
        )
        .child(nav)
        .child(el().key("spacer").grow(1.0))
        .child(text(trf("Version %s", &[&VERSION])).key("version").font(DETAIL).color(p.muted().alpha(0.7)).px(8.0))
}

/// The app's logo; a tile with the update arrows until it is drawn.
fn brand(ui: &Ui, p: Paint) -> El {
    match ui.logo.as_ref().filter(|l| l.is_ready() && l.error().is_none()) {
        Some(logo) => el().key("logo").size(28.0, 28.0).pixels(logo.pixels()).a11y_hidden(),
        None => el().key("logo").size(28.0, 28.0).radius(7.0).center().fill(p.accent).a11y_hidden().child(
            icon(&crate::icons::REFRESH, 15.0).key("glyph").color(Color::WHITE),
        ),
    }
}
