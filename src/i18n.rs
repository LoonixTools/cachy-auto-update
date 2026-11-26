//! Translations.
//!
//! Messages are written in English and looked up in the catalogs that build.rs
//! compiles in from po/. The update run works with LC_ALL=C so that pacman's
//! output stays parseable, so text aimed at a person names its locale: the
//! UI locale here, or the locale of the user a notification goes to.

use std::fmt::Display;
use std::sync::RwLock;

include!(concat!(env!("OUT_DIR"), "/catalogs.rs"));

/// The language picked in the window, over the one of the environment.
static CHOSEN: RwLock<Option<String>> = RwLock::new(None);

/// Native names of the languages, for the language menu, in its order:
/// Latin script by name, then Cyrillic, then the others. Sorting by name
/// alone puts "Čeština" after "Türkçe".
const NAMES: &[(&str, &str)] = &[
    ("en", "English"),
    ("id", "Bahasa Indonesia"),
    ("cs", "Čeština"),
    ("de", "Deutsch"),
    ("es", "Español"),
    ("fr", "Français"),
    ("it", "Italiano"),
    ("hu", "Magyar"),
    ("nl", "Nederlands"),
    ("pl", "Polski"),
    ("pt_BR", "Português (Brasil)"),
    ("ro", "Română"),
    ("sv", "Svenska"),
    ("vi", "Tiếng Việt"),
    ("tr", "Türkçe"),
    ("ru", "Русский"),
    ("uk", "Українська"),
    ("ja", "日本語"),
    ("zh_CN", "简体中文"),
    ("ko", "한국어"),
];

/// Shows the UI and its dates in `lang` ("es", "de_DE.UTF-8", ...) from now
/// on, or in the language of the environment again with `None`.
pub fn set_ui_language(lang: Option<&str>) {
    let lang = lang.map(str::trim).filter(|l| !l.is_empty()).map(String::from);
    crate::status::set_date_language(lang.as_deref());
    *CHOSEN.write().unwrap_or_else(|e| e.into_inner()) = lang;
}

/// The language picked with `set_ui_language`, if any.
pub fn ui_language() -> Option<String> {
    CHOSEN.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The languages there is a catalog for, and English: (code, native name),
/// English first.
pub fn languages() -> Vec<(&'static str, &'static str)> {
    let name = |code: &'static str| NAMES.iter().find(|(c, _)| *c == code).map_or(code, |(_, n)| *n);
    let rank = |code: &str| NAMES.iter().position(|(c, _)| *c == code).unwrap_or(NAMES.len());
    let mut out: Vec<_> = CATALOGS.iter().map(|(code, _)| (*code, name(code))).collect();
    out.push(("en", name("en")));
    out.sort_by_key(|(code, _)| rank(code));
    out
}

/// The locale user-facing text is shown in: the language picked in the
/// window, else the standard POSIX order: LC_ALL wins, and LC_ALL=C really
/// means English.
pub fn ui_locale() -> String {
    if let Some(l) = ui_language() {
        return l;
    }
    env_locale()
}

fn env_locale() -> String {
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

/// The catalog for a locale such as "de_DE.UTF-8", if there is one: one
/// for the language and region ("pt_BR") first, then one for the language.
fn catalog(locale: &str) -> Option<&'static [(&'static str, &'static str)]> {
    let full = locale.split(['.', '@']).next().unwrap_or("");
    let lang = full.split('_').next().unwrap_or("");
    [full, lang].into_iter().find_map(|l| CATALOGS.iter().find(|(c, _)| *c == l).map(|(_, c)| *c))
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

/// `msgid` in the UI locale. The environment is read once per process; the
/// language picked in the window counts from the next call on.
pub fn tr(msgid: &str) -> &str {
    static ENV: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    match CHOSEN.read().unwrap_or_else(|e| e.into_inner()).as_deref() {
        Some(l) => tr_in(l, msgid),
        None => tr_in(ENV.get_or_init(env_locale), msgid),
    }
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

    #[test]
    fn spanish_is_found() {
        assert_eq!(tr_in("es_ES.UTF-8", "ON"), "ACTIVADO");
        assert_eq!(tr_in("es", "Overview"), "Resumen");
        assert_eq!(fill(tr_in("es_MX.UTF-8", "%d updates available"), &[&3]), "3 actualizaciones disponibles");
    }

    #[test]
    fn languages_list_english_first() {
        let l = languages();
        assert_eq!(l[0], ("en", "English"));
        let codes: Vec<_> = l.iter().map(|(c, _)| *c).collect();
        let names: Vec<_> = NAMES.iter().map(|(c, _)| *c).collect();
        assert_eq!(codes, names, "the menu follows NAMES");
        for (code, _) in NAMES {
            let (_, name) = l.iter().find(|(c, _)| c == code).unwrap_or_else(|| panic!("{code} missing"));
            assert_ne!(name, code, "{code} has no native name");
        }
        for (code, _) in CATALOGS {
            assert!(NAMES.iter().any(|(c, _)| c == code), "{code} has no native name");
        }
    }

    #[test]
    fn regions_find_their_catalog() {
        assert_eq!(tr_in("pt_BR.UTF-8", "Settings"), "Configurações");
        assert_eq!(tr_in("zh_CN.UTF-8", "Settings"), "设置");
        // No catalog for Portugal or Taiwan: English, not a near miss.
        assert_eq!(tr_in("pt_PT.UTF-8", "Settings"), "Settings");
        assert_eq!(tr_in("zh_TW.UTF-8", "Settings"), "Settings");
        assert_eq!(tr_in("fr_CA.UTF-8", "Settings"), "Paramètres");
        assert_eq!(tr_in("ja_JP.UTF-8", "Settings"), "設定");
        assert_eq!(tr_in("ko_KR.UTF-8", "Settings"), "설정");
        assert_eq!(tr_in("it_IT.UTF-8", "Settings"), "Impostazioni");
        assert_eq!(tr_in("ru_RU.UTF-8", "Settings"), "Настройки");
        assert_eq!(tr_in("pl_PL.UTF-8", "Settings"), "Ustawienia");
        assert_eq!(tr_in("tr_TR.UTF-8", "Settings"), "Ayarlar");
        assert_eq!(tr_in("nl_BE.UTF-8", "Settings"), "Instellingen");
        assert_eq!(tr_in("uk_UA.UTF-8", "Settings"), "Налаштування");
        assert_eq!(tr_in("cs_CZ.UTF-8", "Settings"), "Nastavení");
        assert_eq!(tr_in("sv_FI.UTF-8", "Settings"), "Inställningar");
        assert_eq!(tr_in("id_ID.UTF-8", "Settings"), "Pengaturan");
        assert_eq!(tr_in("vi_VN.UTF-8", "Settings"), "Cài đặt");
        assert_eq!(tr_in("hu_HU.UTF-8", "Settings"), "Beállítások");
        assert_eq!(tr_in("ro_RO.UTF-8", "Settings"), "Setări");
    }

    #[test]
    fn every_translation_keeps_its_placeholders() {
        fn marks(s: &str) -> Vec<char> {
            let mut out = Vec::new();
            let mut chars = s.chars();
            while let Some(c) = chars.next() {
                if c == '%' && let Some(n @ ('d' | 's' | 'i')) = chars.next() {
                    out.push(n);
                }
            }
            out
        }
        for (lang, catalog) in CATALOGS {
            for (id, text) in *catalog {
                assert_eq!(marks(id), marks(text), "{lang}: {id:?}");
            }
        }
    }

    #[test]
    fn the_picked_language_wins_and_goes_back() {
        set_ui_language(Some("es"));
        assert_eq!(tr("Settings"), "Ajustes");
        assert_eq!(ui_locale(), "es");
        set_ui_language(Some("en"));
        assert_eq!(tr("Settings"), "Settings");
        let day = crate::status::date_time(0);
        assert!(day.starts_with("Wed") || day.starts_with("Thu"), "{day}");
        // No locale of it is installed: the date is in numbers only.
        set_ui_language(Some("xx"));
        assert!(crate::status::date_time(0).starts_with("19"));
        set_ui_language(None);
        assert_eq!(ui_language(), None);
    }
}
