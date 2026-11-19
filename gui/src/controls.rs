//! The settings look of Waypointer: a muted label over a card of rows, on
//! Rune's own design tokens. The switch, the segmented control, the buttons
//! and the dropdown are drawn here, the way Waypointer draws them.

use crate::ui::Ui;
use rune::tok::{self, Family, Theme, TextStyle, motion, radius, space, type_};
use rune::{Align, Color, El, Icon, LoopSpec, Side, Spring, TextEdit, Transition, Tx, col, el, icon, row, text, text_input};

/// Row explanations and other quiet copy.
pub const DETAIL: TextStyle = TextStyle::new(11.5, 400.0);
/// The log's text.
pub const MONO: TextStyle = TextStyle::new(12.0, 400.0).lh(18.0).family(Family::SystemMono);
const ROW_H: f32 = 46.0;
const CONTROL_H: f32 = 30.0;
const SELECT_MIN_W: f32 = 150.0;

/// Everything a control needs to paint itself.
#[derive(Clone, Copy)]
pub struct Paint {
    pub t: Theme,
    pub accent: Color,
    pub hair: f32,
    pub dark: bool,
}

impl Paint {
    pub fn new(dark: bool, scale: f32) -> Self {
        Paint { t: tok::theme(dark), accent: tok::accent(0, dark), hair: 1.0 / scale.max(1.0), dark }
    }
    pub fn text(self) -> Color {
        self.t.text
    }
    pub fn muted(self) -> Color {
        self.t.text_secondary
    }
    /// A condition that lets an update go ahead.
    pub fn good(self) -> Color {
        tok::accent(4, self.dark)
    }
    /// A condition that makes an update wait.
    pub fn wait(self) -> Color {
        tok::accent(3, self.dark)
    }
    pub fn danger(self) -> Color {
        Color { r: 0.95, g: 0.35, b: 0.3, a: 1.0 }
    }
}

pub fn title_row(p: Paint, title: &str, actions: Vec<El>) -> El {
    row()
        .key("title-row")
        .gap(space::S2)
        .center_y()
        .child(text(title).key("title").role("heading").level(1).font(TextStyle::new(22.0, 700.0)).color(p.text()).grow(1.0).truncate())
        .children(actions)
}

/// The muted label over a card.
fn label(p: Paint, title: &str) -> El {
    text(title).key("label").role("heading").level(2).font(type_::SECTION).color(p.muted())
}

/// A card's raised surface.
pub fn surface(p: Paint) -> El {
    col()
        .key("card")
        .fill(p.t.card)
        .radius(radius::LG)
        .border(p.hair, p.t.card_border)
        .shadow(1.0, 2.0, p.t.shadow)
        .shadow(8.0, 24.0, p.t.shadow_soft)
}

/// A card of rows with hairlines between them, under an optional label.
pub fn section(p: Paint, key: &str, title: Option<&str>, rows: Vec<El>) -> El {
    section_note(p, key, title, None, rows)
}

/// A section with a quiet note at the end of its label's line.
pub fn section_note(p: Paint, key: &str, title: Option<&str>, note: Option<&str>, rows: Vec<El>) -> El {
    let mut c = surface(p).padding(space::S1);
    for (i, r) in rows.into_iter().enumerate() {
        if i > 0 {
            c = c.child(el().key(format!("sep{i}")).h(p.hair).ml(space::S3).mr(space::S3).fill(p.t.separator).a11y_hidden());
        }
        c = c.child(r);
    }
    let mut s = col().key(key).gap(space::S2);
    if let Some(title) = title {
        s = s.child(
            row()
                .key("label-row")
                .mx(space::S3)
                .gap(space::S2)
                .center_y()
                .child(label(p, title).grow(1.0))
                .children(note.map(|n| text(n).key("note").font(DETAIL).color(p.muted()))),
        );
    }
    s.child(c)
}

fn words(p: Paint, title: &str, detail: Option<&str>) -> El {
    col()
        .key("words")
        .gap(2.0)
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .child(text(title).key("title").font(type_::BODY).color(p.text()).wrap().a11y_hidden())
        .children(detail.map(|d| text(d).key("detail").font(DETAIL).color(p.muted()).wrap().a11y_hidden()))
}

/// A setting: its words on the left, its control on the right.
pub fn setting(p: Paint, key: &str, title: &str, detail: Option<&str>, control: El) -> El {
    let r = row()
        .key(key)
        .label(title)
        .min_h(ROW_H)
        .px(space::S3)
        .py(8.0)
        .gap(16.0)
        .center_y()
        .justify_between()
        .child(words(p, title, detail))
        .child(control);
    match detail {
        Some(d) => r.description(d),
        None => r,
    }
}

/// A row with an icon at its start.
pub fn icon_row(p: Paint, key: &str, glyph: &'static Icon, title: &str, detail: Option<&str>, control: El) -> El {
    row()
        .key(key)
        .label(title)
        .min_h(ROW_H)
        .px(space::S3)
        .py(8.0)
        .gap(12.0)
        .center_y()
        .child(icon(glyph, 16.0).key("icon").color(p.muted()).a11y_hidden())
        .child(words(p, title, detail))
        .child(control)
}

/// A switch row: the whole row toggles, the switch shows the state. A value
/// on its way to the file is shown at once.
pub fn toggle_row(ui: &Ui, p: Paint, key: &'static str, title: &str, detail: Option<&str>) -> El {
    let on = ui.on(key);
    setting(p, key, title, detail, switch(p, on).a11y_hidden())
        .role("switch")
        .checked(on)
        .radius(radius::MD)
        .hover(p.t.hover)
        .press(p.t.press)
        .focusable(true)
        .on_press(move |s: &mut Ui| s.toggle(key))
}

pub fn switch(p: Paint, on: bool) -> El {
    let (w, h, k) = (40.0, 24.0, 20.0);
    let inset = (h - k) / 2.0;
    el().key("switch").size(w, h).radius(h / 2.0).fill(if on { p.accent } else { p.t.control_off }).motion(Spring::BOUNCY).child(
        el().key("knob")
            .size(k, k)
            .absolute(if on { w - k - inset } else { inset }, inset)
            .radius(k / 2.0)
            .fill(p.t.knob)
            .shadow(1.0, 3.0, p.t.knob_shadow)
            .shadow(0.0, 0.5, p.t.edge),
    )
}

/// A segmented control whose indicator springs to the chosen segment.
/// `width` at least; wider when a label needs it.
pub fn segmented(p: Paint, key: &str, name: &str, labels: &[&str], selected: usize, width: f32, pick: impl Fn(&mut Ui, usize) + Clone + 'static) -> El {
    let (h, pad) = (28.0, 2.0);
    let widest = labels.iter().map(|l| rune::text::measure(l, &[], type_::LABEL, None).0).fold(0.0, f32::max);
    let seg = ((width - 2.0 * pad) / labels.len() as f32).max(widest + 24.0);
    let width = seg * labels.len() as f32 + 2.0 * pad;
    let mut segs = row().key("segments");
    for (i, l) in labels.iter().enumerate() {
        let is = i == selected;
        let pick = pick.clone();
        segs = segs.child(
            el().key(format!("{key}-{i}"))
                .role("segment")
                .label(*l)
                .checked(is)
                .focusable(true)
                .size(seg, h - 2.0 * pad)
                .center()
                .radius(radius::SM)
                .hover(if is { Color::TRANSPARENT } else { p.t.hover })
                .on_press_tx(move |s: &mut Ui, tx: &mut Tx| {
                    pick(s, i);
                    tx.color_motion(motion::CALM);
                })
                .child(text(*l).font(type_::LABEL).color(if is { p.text() } else { p.muted() }).a11y_hidden()),
        );
    }
    el().key(key)
        .role("segmented")
        .label(name)
        .size(width, h)
        .padding(pad)
        .radius(radius::MD)
        .fill(p.t.control_track)
        .motion(motion::SNAPPY)
        .child(
            el().key("indicator")
                .size(seg, h - 2.0 * pad)
                .absolute(pad + selected as f32 * seg, pad)
                .radius(radius::SM)
                .fill(p.t.indicator)
                .shadow(1.0, 3.0, p.t.shadow)
                .shadow(0.0, 0.5, p.t.edge)
                .lead(motion::LEAD, motion::DEFAULT)
                .a11y_hidden(),
        )
        .child(segs)
}

/// A button: the accent colour for the main thing on a page, grey for the rest.
pub fn button(p: Paint, key: &str, label: &str, primary: bool, enabled: bool) -> El {
    let fill = if primary { p.accent } else { p.t.control_off };
    let mut b = el()
        .key(key)
        .role("button")
        .label(label)
        .h(CONTROL_H)
        .px(14.0)
        .center()
        .radius(radius::MD)
        .fill(fill)
        .child(text(label).font(type_::LABEL).color(if primary { Color::WHITE } else { p.text() }).a11y_hidden());
    if enabled {
        b = b.focusable(true).hover(Color::WHITE.alpha(0.06)).press(Color::BLACK.alpha(0.08)).press_scale(0.97);
    } else {
        b = b.disabled(true).opacity(0.5);
    }
    b
}

static CHEVRON: Icon = Icon {
    name: "cau-chevron",
    view: 24.0,
    body: r#"<path d="M6 9l6 6 6-6" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>"#,
};

static CHECK: Icon = Icon {
    name: "cau-check",
    view: 24.0,
    body: r#"<path d="M5 12.5l4.5 4.5L19 7.5" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"/>"#,
};

/// A row that chooses one of a few values. `choices` are (value, label).
/// The open menu goes into `layer`, which the window draws over everything.
pub fn select_row(
    ui: &Ui,
    p: Paint,
    layer: &mut Vec<El>,
    key: &'static str,
    title: &str,
    detail: Option<&str>,
    choices: Vec<(String, String)>,
) -> El {
    let current = ui.value(key);
    // A value from the file that is not among the choices still shows.
    let shown = choices.iter().find(|(v, _)| *v == current).map(|(_, l)| l.clone()).unwrap_or_else(|| current.clone());
    let open = ui.open == Some(key);
    let anchor = format!("{key}-select");
    let trigger = row()
        .key(anchor.clone())
        .role("combobox")
        .label(title)
        .a11y_value(shown.clone())
        .expanded(open)
        .focusable(true)
        .h(CONTROL_H)
        .min_w(SELECT_MIN_W)
        .pl(12.0)
        .pr(8.0)
        .gap(6.0)
        .center_y()
        .radius(radius::MD)
        .fill(p.t.control_off)
        .hover(p.t.hover)
        .press(p.t.press)
        .on_press(move |s: &mut Ui| s.open = if s.open == Some(key) { None } else { Some(key) })
        .child(text(shown).key("value").font(type_::LABEL).color(p.text()).grow(1.0).truncate().a11y_hidden())
        .child(icon(&CHEVRON, 14.0).key("chevron").color(p.muted()).a11y_hidden());
    if open {
        // As wide as its longest label, with the padding, the gap and the tick.
        let widest = choices.iter().map(|(_, l)| rune::text::measure(l, &[], type_::BODY, None).0).fold(0.0, f32::max);
        let width = (widest.ceil() + 52.0).max(SELECT_MIN_W);
        let mut menu = col()
            .key(format!("{key}-menu"))
            .role("listbox")
            .label(title)
            .padding(space::S1)
            .gap(2.0)
            .w(width)
            .radius(radius::LG)
            .fill(p.t.card)
            .border(p.hair, p.t.card_border)
            .shadow(8.0, 24.0, p.t.shadow)
            .transition(Transition::scale(0.96).combined(Transition::opacity()));
        for (v, l) in choices {
            let on = v == current;
            menu = menu.child(
                row()
                    .key(format!("{key}-option-{v}"))
                    .role("option")
                    .label(l.clone())
                    .checked(on)
                    .focusable(true)
                    .h(CONTROL_H)
                    .pl(10.0)
                    .pr(8.0)
                    .gap(10.0)
                    .center_y()
                    .radius(radius::SM)
                    .hover(p.t.hover)
                    .press(p.t.press)
                    .on_press(move |s: &mut Ui| s.set(key, v.clone()))
                    .child(text(l).font(type_::BODY).color(p.text()).grow(1.0).a11y_hidden())
                    .child(if on { icon(&CHECK, 14.0).color(p.accent).a11y_hidden() } else { el().size(14.0, 14.0) }),
            );
        }
        layer.push(menu.anchored(anchor, Side::Bottom, Align::End, 6.0));
    }
    setting(p, key, title, detail, trigger)
}

/// A one-line text field, as Waypointer draws it.
pub fn field(p: Paint, key: &str, edit: &TextEdit, placeholder: &str, width: f32, access: fn(&mut Ui) -> &mut TextEdit, submit: fn(&mut Ui)) -> El {
    let mut f = el()
        .key(key)
        .w(width)
        .h(CONTROL_H + 2.0)
        .px(10.0)
        .center_y()
        .radius(radius::MD)
        .fill(p.t.control_track)
        .border(p.hair, p.t.card_border)
        .child(
            text_input(edit, type_::BODY)
                .single_line()
                .label(placeholder)
                .on_edit(access)
                .on_submit(move |s: &mut Ui, _| submit(s))
                .w_full()
                .color(p.text()),
        );
    if edit.text.is_empty() {
        f = f.child(text(placeholder).font(type_::BODY).color(p.muted()).absolute(10.0, 8.0).a11y_hidden());
    }
    f
}

/// A small coloured dot, for how a condition stands.
pub fn dot(color: Color) -> El {
    el().key("dot").size(8.0, 8.0).radius(4.0).fill(color).a11y_hidden()
}

static RING: Icon = Icon {
    name: "cau-ring",
    view: 16.0,
    body: r#"<circle cx="8" cy="8" r="6" stroke="currentColor" stroke-width="2" fill="none"/>"#,
};

static ARC: Icon = Icon {
    name: "cau-arc",
    view: 16.0,
    body: r#"<path d="M8 2A6 6 0 0 1 13.258 10.89" stroke="currentColor" stroke-width="2" stroke-linecap="round" fill="none"/>"#,
};

/// A ring that turns while something is under way.
pub fn spinner(color: Color, size: f32) -> El {
    el().key("spinner")
        .role("progressbar")
        .label("Loading")
        .size(size, size)
        .child(icon(&RING, size).absolute(0.0, 0.0).color(color).opacity(0.2))
        .child(icon(&ARC, size).absolute(0.0, 0.0).color(color).looping(LoopSpec::rotation(0.9)))
}

/// A bar that fills to `fraction`.
pub fn progress(p: Paint, label: &str, fraction: f32) -> El {
    let f = fraction.clamp(0.0, 1.0);
    el().key("bar")
        .role("progressbar")
        .label(label)
        .range_value(f64::from(f * 100.0), 0.0, 100.0)
        .w_full()
        .h(6.0)
        .radius(3.0)
        .fill(p.t.control_off)
        .child(el().key("fill").w_frac(f).h_full().radius(3.0).fill(p.accent).motion(motion::SNAPPY))
}

/// "every week" as a label of its own: "Every week".
pub fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}
