//! The settings look: raised cards of rows under muted section labels, on
//! rune-kit and the OpenCode theme. Colours, type, radii and shadows come
//! from the kit's tokens.

use crate::ui::Ui;
use rune::{Color, El, TextStyle, col, el, icon, row, text};
use rune_kit::{Kit, Select, Switch};

/// Row descriptions and other quiet copy.
pub const DESCRIPTION: TextStyle = TextStyle::new(12.0, 440.0).lh(16.0);

/// The page's text edge, the gap between sections, and a row's inset in its
/// card. Cards hang outward by `ROW_PX`, so row text lines up with titles.
pub const GUTTER: f32 = 32.0;
pub const SECTION_GAP: f32 = 24.0;
pub const ROW_PX: f32 = 16.0;
const ROW_H: f32 = 48.0;
const SELECT_W: f32 = 200.0;
/// A character of a menu label, a little more than it takes on average.
const MENU_CHAR_W: f32 = 8.0;
/// A menu row around its label: padding, the tick and the gaps.
const MENU_EXTRA_W: f32 = 56.0;

/// Everything a control needs to paint itself.
#[derive(Clone, Copy)]
pub struct Paint {
    pub k: &'static Kit,
    pub hair: f32,
}

impl Paint {
    pub fn new(dark: bool, scale: f32) -> Self {
        Paint { k: rune_theme_opencode::kit(dark), hair: 1.0 / scale.max(1.0) }
    }
    pub fn text(self) -> Color {
        self.k.c.text_base
    }
    pub fn muted(self) -> Color {
        self.k.c.text_muted
    }
    pub fn faint(self) -> Color {
        self.k.c.text_faint
    }
}

/// A page: a column of sections whose cards hang past the gutter.
pub fn page(key: &str) -> El {
    col().key(key).px(GUTTER - ROW_PX).pb(GUTTER).gap(SECTION_GAP)
}

/// A section label over its card; screen readers move between them.
fn label(p: Paint, title: &str) -> El {
    text(title).key("header").role("heading").level(2).font(p.k.ty.ui_medium.lh(16.0)).color(p.muted())
}

/// A titled card of rows.
pub fn section(p: Paint, key: &str, title: &str, rows: Vec<El>) -> El {
    col().key(key).gap(8.0).child(label(p, title).mx(ROW_PX)).child(card(p, "card", rows))
}

/// A titled card with a quiet note at the end of the label's line.
pub fn section_note(p: Paint, key: &str, title: &str, note: &str, rows: Vec<El>) -> El {
    col()
        .key(key)
        .gap(8.0)
        .child(
            row()
                .key("header-row")
                .h(16.0)
                .mx(ROW_PX)
                .gap(8.0)
                .center_y()
                .child(label(p, title).grow(1.0))
                .child(text(note).key("note").font(p.k.ty.small).color(p.faint())),
        )
        .child(card(p, "card", rows))
}

/// The card's raised surface with hairlines between its rows.
pub fn card(p: Paint, key: &str, rows: Vec<El>) -> El {
    let mut c = col().key(key).fill(p.k.c.bg_base).radius(p.k.radius.lg);
    for s in &p.k.elevation.raised {
        c = c.box_shadow(*s);
    }
    for (i, r) in rows.into_iter().enumerate() {
        if i > 0 {
            c = c.child(el().key(format!("sep{i}")).h(p.hair).mx(ROW_PX).fill(p.k.c.border_muted).a11y_hidden());
        }
        c = c.child(r);
    }
    c
}

/// A setting: title and description on the left, its control on the right.
pub fn setting_row(p: Paint, key: &str, title: &str, description: Option<&str>, control: El) -> El {
    let copy = col()
        .key("copy")
        .grow(1.0)
        .shrink()
        .gap(2.0)
        .child(text(title).key("title").font(p.k.ty.ui).color(p.text()).wrap().a11y_hidden())
        .children(description.map(|d| text(d).key("description").font(DESCRIPTION).color(p.muted()).wrap().a11y_hidden()));
    let r = row().key(key).label(title).min_h(ROW_H).px(ROW_PX).py(10.0).gap(16.0).center_y().child(copy).child(control);
    match description {
        Some(d) => r.description(d),
        None => r,
    }
}

/// A row with an icon at its start.
pub fn icon_row(p: Paint, key: &str, glyph: &'static rune::Icon, title: &str, description: Option<&str>, control: El) -> El {
    let lead = icon(glyph, 18.0).key("icon").color(p.k.c.icon_muted).a11y_hidden();
    let copy = col()
        .key("copy")
        .grow(1.0)
        .shrink()
        .gap(2.0)
        .child(text(title).key("title").font(p.k.ty.ui).color(p.text()).wrap().a11y_hidden())
        .children(description.map(|d| text(d).key("description").font(DESCRIPTION).color(p.muted()).wrap().a11y_hidden()));
    row().key(key).label(title).min_h(ROW_H).px(ROW_PX).py(10.0).gap(12.0).center_y().child(lead).child(copy).child(control)
}

/// A switch row: the whole row toggles, the switch shows the state. A value
/// on its way to the file is shown at once.
pub fn toggle_row(ui: &Ui, p: Paint, key: &'static str, title: &str, description: Option<&str>) -> El {
    let on = ui.on(key);
    let switch = Switch::new(on).build(p.k).key("switch").focusable(false).a11y_hidden();
    setting_row(p, key, title, description, switch)
        .role("switch")
        .checked(on)
        .focusable(true)
        .focus_ring(2.0, -2.0, p.k.c.border_focus)
        .on_press(move |s: &mut Ui| s.toggle(key))
}

/// A row that chooses one of a few values. `choices` are (value, label).
pub fn select_row(
    ui: &Ui,
    p: Paint,
    layer: &mut Vec<El>,
    key: &'static str,
    title: &str,
    description: Option<&str>,
    choices: Vec<(String, String)>,
) -> El {
    select_row_with(ui, p, layer, key, title, description, ui.value(key), choices, |s, k, v| s.set(k, v))
}

/// A select row for a value that is not a setting of the file: `set` takes
/// the chosen one.
#[allow(clippy::too_many_arguments)]
pub fn select_row_with(
    ui: &Ui,
    p: Paint,
    layer: &mut Vec<El>,
    key: &'static str,
    title: &str,
    description: Option<&str>,
    current: String,
    choices: Vec<(String, String)>,
    set: fn(&mut Ui, &'static str, String),
) -> El {
    let labels: Vec<String> = choices.iter().map(|(_, l)| l.clone()).collect();
    let shown = choices.iter().find(|(v, _)| *v == current).map(|(_, l)| l.clone()).unwrap_or_else(|| current.clone());
    // A value from the file that is not among the choices still shows.
    let labels = if labels.contains(&shown) { labels } else { [vec![shown.clone()], labels].concat() };
    let select = Select::new(format!("{key}-select"), &labels)
        .current(Some(&shown))
        .label(title)
        .width(SELECT_W)
        .open(ui.open == Some(key))
        .on_toggle(move |s: &mut Ui| s.open = if s.open == Some(key) { None } else { Some(key) })
        .on_close(|s: &mut Ui| s.open = None)
        .on_choose(move |s: &mut Ui, label: &str, _| {
            if let Some((v, _)) = choices.iter().find(|(_, l)| l == label) {
                set(s, key, v.clone());
            } else {
                s.open = None;
            }
        })
        .build(p.k, layer);
    // Rune's menu does not grow for its labels: they shrink and get cut. So
    // it gets the width of the longest one. The menu is what it put last.
    if ui.open == Some(key)
        && let Some(menu) = layer.pop()
    {
        let longest = labels.iter().map(|l| l.chars().count()).max().unwrap_or(0) as f32;
        layer.push(menu.min_w(menu_width(longest)));
    }
    setting_row(p, key, title, description, select)
}

/// A menu wide enough for a label of `chars` characters.
pub fn menu_width(chars: f32) -> f32 {
    (chars * MENU_CHAR_W + MENU_EXTRA_W).clamp(160.0, 368.0)
}

/// A small coloured dot, for how a condition stands.
pub fn dot(color: Color) -> El {
    el().key("dot").size(8.0, 8.0).radius(4.0).fill(color).a11y_hidden()
}

/// "every week" as a label of its own: "Every week".
pub fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}
