//! The language of the window, picked by each user for themselves. It lives
//! in ~/.config/cachy-auto-update/language; an empty or missing file means
//! the language of the desktop.

use crate::controls::{Paint, page, section, select_row_with};
use crate::ui::Ui;
use cachy_auto_update::i18n::{languages, tr};
use rune::El;
use std::path::PathBuf;

fn file() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join(cachy_auto_update::paths::NAME).join("language"))
}

/// The saved language, "" for the one of the desktop.
pub fn load() -> String {
    let saved = file().and_then(|f| std::fs::read_to_string(f).ok()).unwrap_or_default();
    let saved = saved.trim();
    // A language this version has no catalog for counts as none.
    if languages().iter().any(|(code, _)| *code == saved) { saved.to_string() } else { String::new() }
}

pub fn save(lang: &str) -> std::io::Result<()> {
    let Some(f) = file() else { return Err(std::io::ErrorKind::NotFound.into()) };
    if let Some(dir) = f.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(f, format!("{lang}\n"))
}

pub fn pane(ui: &Ui, p: Paint, layer: &mut Vec<El>) -> El {
    let mut choices = vec![(String::new(), tr("System default").to_string())];
    choices.extend(languages().into_iter().map(|(code, name)| (code.to_string(), name.to_string())));
    page("language").child(section(
        p,
        "window",
        tr("Window"),
        vec![select_row_with(
            ui,
            p,
            layer,
            "Language",
            tr("Language of this window"),
            Some(tr("Just for you. Notifications follow the desktop.")),
            ui.language.clone(),
            choices,
            |s, _, v| s.set_language(v),
        )],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_language_comes_back() {
        let dir = std::env::temp_dir().join(format!("cau-lang-{}", std::process::id()));
        // The only test that touches the environment.
        unsafe { std::env::set_var("XDG_CONFIG_HOME", &dir) };
        assert_eq!(load(), "");
        save("es").unwrap();
        assert_eq!(load(), "es");
        save("").unwrap();
        assert_eq!(load(), "");
        // A language without a catalog counts as none.
        std::fs::write(file().unwrap(), "xx\n").unwrap();
        assert_eq!(load(), "");
        let _ = std::fs::remove_dir_all(dir);
    }
}
