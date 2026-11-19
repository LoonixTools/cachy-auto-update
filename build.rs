//! Compiles the gettext catalogs in po/ into the binary, so translations need
//! no files at run time and no libintl.

use std::{env, fs, path::Path};

/// One string of a .po file, with its escapes undone.
fn unquote(s: &str) -> String {
    let s = s.trim();
    let s = s.strip_prefix('"').and_then(|s| s.strip_suffix('"')).unwrap_or("");
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => {}
        }
    }
    out
}

/// The msgid and msgstr pairs of a .po file. Fuzzy and untranslated entries
/// are left out, so they fall back to English.
fn parse(src: &str) -> Vec<(String, String)> {
    #[derive(PartialEq)]
    enum Field {
        None,
        Id,
        Str,
    }
    let mut out = Vec::new();
    let (mut id, mut text, mut field, mut fuzzy) = (String::new(), String::new(), Field::None, false);
    let mut flush = |id: &mut String, text: &mut String, fuzzy: &mut bool| {
        if !id.is_empty() && !text.is_empty() && !*fuzzy {
            out.push((std::mem::take(id), std::mem::take(text)));
        }
        id.clear();
        text.clear();
        *fuzzy = false;
    };
    for line in src.lines() {
        let line = line.trim();
        if line.starts_with("#,") && line.contains("fuzzy") {
            fuzzy = true;
        } else if let Some(rest) = line.strip_prefix("msgid ") {
            if field == Field::Str {
                flush(&mut id, &mut text, &mut fuzzy);
            }
            id = unquote(rest);
            field = Field::Id;
        } else if let Some(rest) = line.strip_prefix("msgstr ") {
            text = unquote(rest);
            field = Field::Str;
        } else if line.starts_with('"') {
            match field {
                Field::Id => id.push_str(&unquote(line)),
                Field::Str => text.push_str(&unquote(line)),
                Field::None => {}
            }
        } else if line.is_empty() && field == Field::Str {
            flush(&mut id, &mut text, &mut fuzzy);
            field = Field::None;
        }
    }
    if field == Field::Str {
        flush(&mut id, &mut text, &mut fuzzy);
    }
    out
}

fn main() {
    println!("cargo:rerun-if-changed=po");
    let mut code = String::new();
    let mut langs = Vec::new();
    let mut files: Vec<_> = fs::read_dir("po")
        .map(|d| d.filter_map(Result::ok).map(|e| e.path()).collect())
        .unwrap_or_default();
    files.sort();
    for path in files {
        if path.extension().and_then(|e| e.to_str()) != Some("po") {
            continue;
        }
        println!("cargo:rerun-if-changed={}", path.display());
        let lang = path.file_stem().unwrap().to_string_lossy().to_string();
        let mut entries = parse(&fs::read_to_string(&path).unwrap());
        entries.sort();
        entries.dedup_by(|a, b| a.0 == b.0);
        let name = format!("CATALOG_{}", lang.to_uppercase().replace(['-', '@'], "_"));
        code.push_str(&format!("static {name}: &[(&str, &str)] = &[\n"));
        for (id, text) in &entries {
            code.push_str(&format!("    ({id:?}, {text:?}),\n"));
        }
        code.push_str("];\n");
        langs.push((lang, name));
    }
    code.push_str("pub(crate) static CATALOGS: &[(&str, &[(&str, &str)])] = &[\n");
    for (lang, name) in &langs {
        code.push_str(&format!("    ({lang:?}, {name}),\n"));
    }
    code.push_str("];\n");
    let out = Path::new(&env::var("OUT_DIR").unwrap()).join("catalogs.rs");
    fs::write(out, code).unwrap();
}
