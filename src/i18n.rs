//! Translations.
//!
//! Messages are written in English and looked up in the catalogs that build.rs
//! compiles in from po/. The update run works with LC_ALL=C so that pacman's
//! output stays parseable, so text aimed at a person names its locale: the
//! UI locale here, or the locale of the user a notification goes to.

use std::fmt::Display;

include!(concat!(env!("OUT_DIR"), "/catalogs.rs"));

/// The locale user-facing text is shown in. Standard POSIX order: LC_ALL wins,
/// and LC_ALL=C really means English.
pub fn ui_locale() -> String {
    for var in ["CAU_UI_LOCALE", "LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(v) = std::env::var(var)
            && !v.is_empty()
        {
            return v;
        }
    }
    system_locale().unwrap_or_else(|| "C".into())
}

/// LANG from /etc/locale.conf.
pub fn system_locale() -> Option<String> {
    let text = std::fs::read_to_string("/etc/locale.conf").ok()?;
    text.lines()
        .find_map(|l| l.strip_prefix("LANG="))
        .map(|v| v.trim().trim_matches('"').to_string())
        .filter(|v| !v.is_empty())
}

/// The catalog for a locale such as "de_DE.UTF-8", if there is one.
fn catalog(locale: &str) -> Option<&'static [(&'static str, &'static str)]> {
    let lang = locale.split(['_', '.', '@']).next().unwrap_or("");
    CATALOGS.iter().find(|(l, _)| *l == lang).map(|(_, c)| *c)
}

/// `msgid` in `locale`, or `msgid` itself when it has no translation.
pub fn tr_in<'a>(locale: &str, msgid: &'a str) -> &'a str {
    match catalog(locale) {
        Some(c) => match c.binary_search_by(|(id, _)| (*id).cmp(msgid)) {
            Ok(i) => c[i].1,
            Err(_) => msgid,
        },
        None => msgid,
    }
}

/// `msgid` in the UI locale. The locale is read once per process.
pub fn tr(msgid: &str) -> &str {
    static LOCALE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    tr_in(LOCALE.get_or_init(ui_locale), msgid)
}

/// Fills a printf-style message: each `%d`, `%s` or `%i` takes the next
/// argument, `%%` is a percent sign. Anything else stays as it is, so a
/// translation with a stray % cannot break the text.
pub fn fill(template: &str, args: &[&dyn Display]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut args = args.iter();
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('%') => {
                chars.next();
                out.push('%');
            }
            Some('d' | 's' | 'i') => {
                chars.next();
                match args.next() {
                    Some(a) => out.push_str(&a.to_string()),
                    None => out.push('?'),
                }
            }
            _ => out.push('%'),
        }
    }
    out
}

/// `tr` and `fill` in one.
pub fn trf(msgid: &str, args: &[&dyn Display]) -> String {
    fill(tr(msgid), args)
}

/// `tr_in` and `fill` in one.
pub fn trf_in(locale: &str, msgid: &str, args: &[&dyn Display]) -> String {
    fill(tr_in(locale, msgid), args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_in_order_and_keeps_percent() {
        assert_eq!(fill("battery at %d%%, below %d%%", &[&20, &30]), "battery at 20%, below 30%");
        assert_eq!(fill("%s could not", &[&"foo"]), "foo could not");
        assert_eq!(fill("100 % done", &[]), "100 % done");
    }

    #[test]
    fn german_is_found() {
        assert_eq!(tr_in("de_DE.UTF-8", "ON"), "AN");
        assert_eq!(tr_in("C", "ON"), "ON");
        assert_eq!(tr_in("de_DE.UTF-8", "no such message"), "no such message");
    }
}
