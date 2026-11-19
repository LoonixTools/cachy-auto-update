//! /etc/cachy-auto-update/cachy-auto-update.conf
//!
//! One "Key=Value" per line, '#' starts a comment, the last assignment wins.
//! The file is parsed, never run, so nothing in it can become code.

use crate::paths::{NAME, paths};
use std::collections::HashMap;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// What kind of value a setting takes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Bool,
    /// One of these values, in this order.
    Choice(&'static [&'static str]),
    /// A choice of durations, shown as "every week".
    Interval(&'static [&'static str]),
    /// "any" or hours like "3-6". These are offered, any others are taken.
    Hours(&'static [&'static str]),
    /// Package names separated by spaces.
    Packages,
}

pub struct Setting {
    pub key: &'static str,
    pub kind: Kind,
    pub default: &'static str,
    /// The label, as a msgid.
    pub label: &'static str,
}

const fn s(key: &'static str, kind: Kind, default: &'static str, label: &'static str) -> Setting {
    Setting { key, kind, default, label }
}

pub const AUR_HELPERS: &[&str] = &["auto", "paru", "yay", "pikaur"];

/// When the computer restarts by itself after an update.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Reboot {
    Never,
    /// After an update that changed the kernel.
    Needed,
    /// After every update that installed something.
    Always,
}

impl Reboot {
    /// From the file. "yes" means the careful choice.
    pub fn parse(v: &str) -> Self {
        match v.to_ascii_lowercase().as_str() {
            "always" => Reboot::Always,
            "needed" => Reboot::Needed,
            v if is_true(v) => Reboot::Needed,
            _ => Reboot::Never,
        }
    }
}

/// The hours in which an update may start, in local time: from `from`:00 up
/// to `until`:00. They may wrap past midnight, as 22-6 does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hours {
    pub from: u32,
    pub until: u32,
}

/// A value for UpdateHours that is neither "any" nor hours.
#[derive(Debug, PartialEq)]
pub struct BadHours;

impl Hours {
    /// "any" is Ok(None). "3-6" is 03:00 to 06:00, and 24 is midnight.
    pub fn parse(v: &str) -> Result<Option<Hours>, BadHours> {
        let v = v.trim();
        if v.eq_ignore_ascii_case("any") {
            return Ok(None);
        }
        let (a, b) = v.split_once('-').ok_or(BadHours)?;
        let hour = |s: &str| s.trim().parse::<u32>().ok().filter(|h| *h <= 24).map(|h| h % 24).ok_or(BadHours);
        let h = Hours { from: hour(a)?, until: hour(b)? };
        if h.from == h.until {
            return Err(BadHours);
        }
        Ok(Some(h))
    }

    pub fn contains(&self, hour: u32) -> bool {
        if self.from < self.until { (self.from..self.until).contains(&hour) } else { hour >= self.from || hour < self.until }
    }
}

/// Every setting there is.
pub const SETTINGS: &[Setting] = &[
    s("Enabled", Kind::Bool, "no", "Automatic updates"),
    s("Notifications", Kind::Bool, "yes", "Notifications"),
    s("UpdateInterval", Kind::Interval(&["1d", "2d", "3d", "1w", "2w", "4w"]), "1w", "How often to update"),
    s(
        "UpdateHours",
        Kind::Hours(&["any", "0-3", "3-6", "6-9", "9-12", "12-15", "15-18", "18-21", "21-24"]),
        "any",
        "When to update",
    ),
    s("AutoReboot", Kind::Choice(&["no", "needed", "always"]), "no", "Restart automatically"),
    s("TrayIcon", Kind::Bool, "yes", "Show an icon in the system tray"),
    s("CachyUpdate", Kind::Bool, "no", "Also run cachy-update"),
    s("RebootNotification", Kind::Bool, "no", "Show the CachyOS restart notice"),
    s("NotifyOnStart", Kind::Bool, "yes", "Notify when an update starts"),
    s("MinimizeProgress", Kind::Bool, "no", "Start the progress bar minimized"),
    s("NotifyOnSuccess", Kind::Bool, "yes", "Notify after a successful update"),
    s("NotifyOnError", Kind::Bool, "yes", "Notify when something goes wrong"),
    s("SkipWhenGaming", Kind::Bool, "yes", "Postpone while a game is running"),
    s("RequireAC", Kind::Bool, "no", "Only update on mains power"),
    s("MinBatteryPercent", Kind::Choice(&["0", "20", "30", "40", "50", "60", "70", "80"]), "30", "Minimum battery level (%)"),
    s("UpdateAUR", Kind::Bool, "yes", "Update AUR packages"),
    s("UpdateFlatpak", Kind::Bool, "yes", "Update Flatpaks"),
    s("UpdateAppImages", Kind::Bool, "yes", "Update AppImages"),
    s("UpdateDevel", Kind::Bool, "no", "Also rebuild -git packages"),
    s("AURHelper", Kind::Choice(AUR_HELPERS), "auto", "AUR helper"),
    s("AutoResolveConflicts", Kind::Bool, "yes", "Resolve package conflicts automatically"),
    s("CleanCache", Kind::Bool, "yes", "Trim the package cache"),
    s("KeepOldPackages", Kind::Choice(&["0", "1", "2", "3", "5"]), "3", "Cached versions to keep"),
    s("CleanAURCache", Kind::Bool, "yes", "Clean the AUR build caches"),
    s("RemoveOrphans", Kind::Bool, "no", "Remove packages nothing needs any more"),
    s("IgnorePkg", Kind::Packages, "", "Never update these packages"),
];

pub fn setting(key: &str) -> Option<&'static Setting> {
    SETTINGS.iter().find(|s| s.key.eq_ignore_ascii_case(key))
}

pub fn is_true(v: &str) -> bool {
    matches!(v.to_ascii_lowercase().as_str(), "yes" | "y" | "true" | "1" | "on" | "enabled")
}

fn is_false(v: &str) -> bool {
    matches!(v.to_ascii_lowercase().as_str(), "no" | "n" | "false" | "0" | "off" | "disabled")
}

/// Whether a name is safe as a package name.
pub fn is_package_name(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "@._+-".contains(c))
}

/// "1d", "6h", "30m" or a number of seconds, in seconds.
pub fn duration_seconds(v: &str) -> Option<u64> {
    let unit = v.chars().last()?;
    let (num, mult) = match unit {
        's' => (&v[..v.len() - 1], 1),
        'm' => (&v[..v.len() - 1], 60),
        'h' => (&v[..v.len() - 1], 3600),
        'd' => (&v[..v.len() - 1], 86400),
        'w' => (&v[..v.len() - 1], 604800),
        c if c.is_ascii_digit() => (v, 1),
        _ => return None,
    };
    if num.is_empty() || !num.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    num.parse::<u64>().ok()?.checked_mul(mult)
}

/// The value as the file should hold it, or an error to show. Only known
/// keys with values of the right shape get in: some of them end up on a
/// command line run as root.
pub fn normalize(key: &str, value: &str) -> Result<(&'static str, String), String> {
    let Some(set) = setting(key) else {
        return Err(crate::i18n::trf("Unknown setting: %s", &[&key]));
    };
    let value = value.trim();
    let bad = || crate::i18n::trf("Not a valid value for %s: %s", &[&set.key, &value]);
    let v = match set.kind {
        Kind::Bool => {
            if is_true(value) {
                "yes".to_string()
            } else if is_false(value) {
                "no".to_string()
            } else {
                return Err(bad());
            }
        }
        Kind::Choice(choices) => {
            if choices.contains(&value) {
                value.to_string()
            } else if choices.iter().all(|c| c.chars().all(|c| c.is_ascii_digit()))
                && value.chars().all(|c| c.is_ascii_digit())
                && !value.is_empty()
                && value.len() <= 3
            {
                // A number other than the ones offered is still a number.
                value.to_string()
            } else {
                return Err(bad());
            }
        }
        Kind::Interval(_) => match duration_seconds(value) {
            Some(n) if n > 0 => value.to_string(),
            _ => return Err(bad()),
        },
        Kind::Hours(_) => match Hours::parse(value) {
            Ok(None) => "any".to_string(),
            Ok(Some(h)) => format!("{}-{}", h.from, if h.until == 0 { 24 } else { h.until }),
            Err(BadHours) => return Err(bad()),
        },
        Kind::Packages => {
            let names: Vec<&str> = value.split_whitespace().collect();
            if names.iter().all(|n| is_package_name(n)) {
                names.join(" ")
            } else {
                return Err(bad());
            }
        }
    };
    Ok((set.key, v))
}

/// The file's values. Values are unquoted and their comments stripped.
pub fn parse(text: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else { continue };
        let key = key.trim();
        if key.is_empty() || key.starts_with('#') || key.contains(char::is_whitespace) {
            continue;
        }
        let value = value.split('#').next().unwrap_or("").trim();
        let value = value.strip_prefix('"').unwrap_or(value);
        let value = value.strip_suffix('"').unwrap_or(value);
        if value.is_empty() {
            // An empty value means the default, as in the shell version.
            map.remove(key);
        } else {
            map.insert(key.to_string(), value.to_string());
        }
    }
    map
}

pub fn read_raw() -> HashMap<String, String> {
    std::fs::read_to_string(&paths().config).map(|t| parse(&t)).unwrap_or_default()
}

/// Puts `key=value` into the text: in place of the first line that sets it
/// (a commented-out template line counts), else at the end. Later lines for
/// the same key go, so the file cannot grow duplicates.
pub fn replace_in(text: &str, key: &str, value: &str) -> String {
    let sets = |line: &str, commented: bool| {
        let l = line.trim_start();
        let l = if commented { l.strip_prefix('#').map(str::trim_start).unwrap_or(l) } else { l };
        l.strip_prefix(key).is_some_and(|rest| rest.trim_start().starts_with('='))
    };
    let mut out = String::with_capacity(text.len() + 32);
    let mut done = false;
    for line in text.lines() {
        if !done && sets(line, true) {
            out.push_str(&format!("{key}={value}\n"));
            done = true;
        } else if done && sets(line, false) {
            continue;
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    if !done {
        out.push_str(&format!("{key}={value}\n"));
    }
    out
}

/// Writes one setting. Atomic: a temporary file next to it is renamed over
/// it, with the old file's permissions.
pub fn set(key: &str, value: &str) -> std::io::Result<()> {
    let path = &paths().config;
    let old = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            format!("# {NAME} configuration\n")
        }
        Err(e) => return Err(e),
    };
    write_atomic(path, replace_in(&old, key, value).as_bytes())
}

pub fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let mode = std::fs::metadata(path).map(|m| m.permissions().mode() & 0o7777).unwrap_or(0o644);
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.set_permissions(std::fs::Permissions::from_mode(mode))?;
        f.write_all(data)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// The settings, resolved. Read once per process and passed around.
#[derive(Clone, Debug)]
pub struct Config {
    pub raw: HashMap<String, String>,
    pub enabled: bool,
    pub notifications: bool,
    pub require_ac: bool,
    pub skip_gaming: bool,
    pub aur: bool,
    pub flatpak: bool,
    pub appimage: bool,
    pub devel: bool,
    pub resolve_conflicts: bool,
    pub clean_cache: bool,
    pub remove_orphans: bool,
    pub clean_aur_cache: bool,
    pub notify_start: bool,
    pub minimize_progress: bool,
    pub notify_success: bool,
    pub notify_error: bool,
    pub tray: bool,
    pub cachy_update: bool,
    pub reboot_notify: bool,
    pub auto_reboot: Reboot,
    pub interval: String,
    pub interval_seconds: u64,
    /// None: any time.
    pub hours: Option<Hours>,
    pub min_battery: i64,
    pub keep_old: i64,
    pub aur_helper: String,
    pub ignore_pkg: Vec<String>,
}

impl Config {
    pub fn load() -> Self {
        Self::from_raw(read_raw())
    }

    /// The value of a key as the file has it, else its default.
    pub fn get(&self, key: &str) -> String {
        if let Some(v) = self.raw.get(key) {
            return v.clone();
        }
        setting(key).map(|s| s.default.to_string()).unwrap_or_default()
    }

    pub fn from_raw(raw: HashMap<String, String>) -> Self {
        let get = |k: &str, d: &str| raw.get(k).cloned().unwrap_or_else(|| d.to_string());
        let b = |k: &str, d: &str| is_true(&get(k, d));
        let int = |k: &str, d: i64| get(k, "").parse::<i64>().unwrap_or(d);
        let interval = get("UpdateInterval", "1w");
        let helper = get("AURHelper", "auto");
        Config {
            enabled: b("Enabled", "no"),
            notifications: b("Notifications", "yes"),
            require_ac: b("RequireAC", "no"),
            skip_gaming: b("SkipWhenGaming", "yes"),
            aur: b("UpdateAUR", "yes"),
            flatpak: b("UpdateFlatpak", "yes"),
            appimage: b("UpdateAppImages", "yes"),
            devel: b("UpdateDevel", "no"),
            resolve_conflicts: b("AutoResolveConflicts", "yes"),
            clean_cache: b("CleanCache", "yes"),
            remove_orphans: b("RemoveOrphans", "no"),
            clean_aur_cache: b("CleanAURCache", "yes"),
            notify_start: b("NotifyOnStart", "yes"),
            minimize_progress: b("MinimizeProgress", "no"),
            notify_success: b("NotifyOnSuccess", "yes"),
            notify_error: b("NotifyOnError", "yes"),
            tray: b("TrayIcon", "yes"),
            cachy_update: b("CachyUpdate", "no"),
            reboot_notify: b("RebootNotification", "no"),
            auto_reboot: Reboot::parse(&get("AutoReboot", "no")),
            interval_seconds: duration_seconds(&interval).filter(|n| *n > 0).unwrap_or(604800),
            interval,
            // A value that makes no sense means any time, not never.
            hours: Hours::parse(&get("UpdateHours", "any")).ok().flatten(),
            min_battery: int("MinBatteryPercent", 30),
            keep_old: int("KeepOldPackages", 3).max(0),
            // Only a helper this program knows: the name is run as a command.
            aur_helper: if AUR_HELPERS.contains(&helper.as_str()) { helper } else { "auto".into() },
            ignore_pkg: get("IgnorePkg", "").split_whitespace().filter(|n| is_package_name(n)).map(String::from).collect(),
            raw,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_one_wins_and_comments_go() {
        let m = parse("A=1\n# B=2\nA = 3 # three\nC=\"x\"\nD=\n");
        assert_eq!(m.get("A").unwrap(), "3");
        assert_eq!(m.get("C").unwrap(), "x");
        assert!(!m.contains_key("B"));
        assert!(!m.contains_key("D"));
    }

    #[test]
    fn replaces_the_template_line_and_drops_duplicates() {
        let t = "# Enabled=no\nX=1\nEnabled=yes\n";
        assert_eq!(replace_in(t, "Enabled", "no"), "Enabled=no\nX=1\n");
        assert_eq!(replace_in("X=1\n", "Enabled", "yes"), "X=1\nEnabled=yes\n");
        // EnabledFoo is another key.
        assert_eq!(replace_in("EnabledFoo=1\n", "Enabled", "yes"), "EnabledFoo=1\nEnabled=yes\n");
    }

    #[test]
    fn durations() {
        assert_eq!(duration_seconds("1w"), Some(604800));
        assert_eq!(duration_seconds("90"), Some(90));
        assert_eq!(duration_seconds("2x"), None);
        assert_eq!(duration_seconds("d"), None);
    }

    #[test]
    fn values_are_checked() {
        assert_eq!(normalize("enabled", "on").unwrap(), ("Enabled", "yes".into()));
        assert!(normalize("AURHelper", "/tmp/evil").is_err());
        assert!(normalize("IgnorePkg", "linux; rm").is_err());
        assert_eq!(normalize("IgnorePkg", " linux  nvidia ").unwrap().1, "linux nvidia");
        assert!(normalize("Nope", "1").is_err());
        assert_eq!(normalize("MinBatteryPercent", "25").unwrap().1, "25");
        assert_eq!(normalize("AutoReboot", "needed").unwrap(), ("AutoReboot", "needed".into()));
        assert!(normalize("AutoReboot", "reboot; rm").is_err());
    }

    #[test]
    fn hours() {
        assert_eq!(Hours::parse("any"), Ok(None));
        let night = Hours::parse("22-6").unwrap().unwrap();
        assert!(night.contains(23) && night.contains(0) && night.contains(5));
        assert!(!night.contains(6) && !night.contains(21));
        let early = Hours::parse("3-6").unwrap().unwrap();
        assert!(early.contains(3) && early.contains(5) && !early.contains(6) && !early.contains(2));
        assert!(Hours::parse("21-24").unwrap().unwrap().contains(23));
        assert!(Hours::parse("4-4").is_err() && Hours::parse("25-3").is_err() && Hours::parse("4").is_err());
        assert_eq!(normalize("UpdateHours", " 04-06 ").unwrap().1, "4-6");
        assert_eq!(normalize("UpdateHours", "21-0").unwrap().1, "21-24");
        assert_eq!(normalize("UpdateHours", "ANY").unwrap().1, "any");
        assert!(normalize("UpdateHours", "4-6; rm").is_err());
        // Nonsense in the file means any time, never no updates at all.
        assert_eq!(Config::from_raw(HashMap::from([("UpdateHours".into(), "soon".into())])).hours, None);
    }

    #[test]
    fn restart_is_off_unless_asked() {
        assert_eq!(Config::from_raw(HashMap::new()).auto_reboot, Reboot::Never);
        assert_eq!(Reboot::parse("Always"), Reboot::Always);
        assert_eq!(Reboot::parse("yes"), Reboot::Needed);
        assert_eq!(Reboot::parse("maybe"), Reboot::Never);
    }
}
