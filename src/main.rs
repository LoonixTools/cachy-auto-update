//! cachy-auto-update: the command line.
//!
//! Everything it does is flipping a switch in the configuration, driving the
//! systemd timer, or the update run itself. Without a command it opens the
//! window.

use cachy_auto_update::config::{self, Config};
use cachy_auto_update::i18n::{tr, trf, ui_locale};
use cachy_auto_update::paths::{NAME, PRETTY, VERSION, paths, self_exe};
use cachy_auto_update::status::{self, LastResult, Next, Status};
use cachy_auto_update::{conditions, locks, log, notify, proc, quiet, runner, users};
use std::os::unix::process::CommandExt;
use std::process::Command;

struct Colors {
    reset: &'static str,
    bold: &'static str,
    dim: &'static str,
    blue: &'static str,
    green: &'static str,
    yellow: &'static str,
    red: &'static str,
}

fn colors() -> &'static Colors {
    static ON: Colors = Colors {
        reset: "\x1b[0m",
        bold: "\x1b[1m",
        dim: "\x1b[2m",
        blue: "\x1b[38;2;23;147;209m",
        green: "\x1b[32m",
        yellow: "\x1b[33m",
        red: "\x1b[31m",
    };
    static OFF: Colors = Colors { reset: "", bold: "", dim: "", blue: "", green: "", yellow: "", red: "" };
    if log::interactive() && std::env::var_os("NO_COLOR").is_none() { &ON } else { &OFF }
}

fn ok(msg: &str) {
    let c = colors();
    println!("{}✔{} {msg}", c.green, c.reset);
}

fn bad(msg: &str) {
    let c = colors();
    eprintln!("{}✘{} {msg}", c.red, c.reset);
}

fn hint(msg: &str) {
    let c = colors();
    println!("{}•{} {msg}", c.dim, c.reset);
}

/// Runs again with sudo when a person is at the terminal, so typing the
/// command just works. Else it says root is needed and exits.
fn need_root() {
    if proc::is_root() {
        return;
    }
    use std::io::IsTerminal;
    if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
        for elevate in ["sudo", "run0", "doas"] {
            if proc::have(elevate) {
                let err = Command::new(elevate).arg(self_exe()).args(std::env::args().skip(1)).exec();
                bad(&err.to_string());
                std::process::exit(2);
            }
        }
    }
    bad(tr("This command has to run as root."));
    std::process::exit(2);
}

/// The window and tray program, next to this one.
fn gui_exe() -> std::path::PathBuf {
    self_exe().with_file_name("cachy-auto-update-gui")
}

/// Shows or hides the tray icon now, in every session, instead of at the
/// next login. An icon switched off notices by itself and goes, so only
/// starting one is needed here. Through the user's own service manager, so
/// the icon gets the session's environment and not root's.
fn tray_apply(cfg: &Config) {
    if !cfg.tray || !proc::is_root() || !gui_exe().exists() {
        return;
    }
    for user in users::session_users() {
        proc::quiet(
            users::as_user(&user, "systemd-run")
                .args(["--user", "--quiet", "--collect", "--unit=cachy-auto-update-tray"])
                .arg(gui_exe())
                .arg("--tray"),
        );
    }
}

fn set(key: &str, value: &str) -> bool {
    if config::set(key, value).is_err() {
        bad(tr("Could not write the configuration file."));
        return false;
    }
    true
}

fn enable() -> i32 {
    need_root();
    // A broken sudoers file would take sudo down for the whole machine, so it
    // is checked before anything is switched on.
    let sudoers = "/etc/sudoers.d/cachy-auto-update";
    if std::path::Path::new(sudoers).is_file()
        && proc::have("visudo")
        && !proc::quiet(Command::new("visudo").args(["-cf", sudoers]))
    {
        bad(tr("/etc/sudoers.d/cachy-auto-update is invalid; refusing to enable."));
        return 1;
    }
    if !set("Enabled", "yes") {
        return 1;
    }
    if proc::have("systemctl")
        && !proc::quiet(Command::new("systemctl").args(["enable", "--now", "cachy-auto-update.timer"]))
    {
        bad(tr("Could not enable the systemd timer."));
    }
    ok(tr("Automatic updates are on."));
    let cfg = Config::load();
    if cfg.aur && !cachy_auto_update::pkg::aur::helper_installed(&cfg) {
        hint(tr("No AUR helper found. Install paru or yay for AUR updates."));
    }
    if cfg.aur && !proc::have("makepkg") {
        hint(tr("base-devel is missing, so AUR packages cannot be built."));
    }
    // cachy-update and the CachyOS restart notice go quiet (see quiet.rs).
    quiet::apply(&cfg, false);
    tray_apply(&cfg);
    0
}

fn disable() -> i32 {
    need_root();
    if !set("Enabled", "no") {
        return 1;
    }
    if proc::have("systemctl") {
        proc::quiet(Command::new("systemctl").args(["disable", "--now", "cachy-auto-update.timer"]));
    }
    ok(tr("Automatic updates are off."));
    // Without automatic updates, cachy-update and the restart notice are
    // useful again.
    quiet::apply(&Config::load(), false);
    0
}

fn notifications(arg: Option<&str>) -> i32 {
    need_root();
    match arg.map(str::to_ascii_lowercase).as_deref() {
        Some("on" | "yes" | "true" | "1") => {
            if !set("Notifications", "yes") {
                return 1;
            }
            ok(tr("Notifications are on."));
            0
        }
        Some("off" | "no" | "false" | "0") => {
            if !set("Notifications", "no") {
                return 1;
            }
            ok(tr("Notifications are off."));
            0
        }
        _ => {
            bad(tr("Usage: cachy-auto-update notifications on|off"));
            1
        }
    }
}

/// `config [list | get KEY | set KEY VALUE]`. Settings that change something
/// outside the file take effect at once.
fn config_cmd(args: &[String]) -> i32 {
    match args {
        [] => config_cmd(&["list".into()]),
        [cmd] if cmd == "list" => {
            let cfg = Config::load();
            for s in config::SETTINGS {
                println!("{}={}", s.key, cfg.get(s.key));
            }
            0
        }
        [cmd, key] if cmd == "get" => match config::setting(key) {
            Some(s) => {
                println!("{}", Config::load().get(s.key));
                0
            }
            None => {
                bad(&trf("Unknown setting: %s", &[key]));
                1
            }
        },
        [cmd, key, value @ ..] if cmd == "set" => {
            let value = value.join(" ");
            let (key, value) = match config::normalize(key, &value) {
                Ok(kv) => kv,
                Err(e) => {
                    bad(&e);
                    return 1;
                }
            };
            match key {
                "Enabled" if value == "yes" => return enable(),
                "Enabled" => return disable(),
                _ => {}
            }
            need_root();
            if !set(key, &value) {
                return 1;
            }
            let cfg = Config::load();
            match key {
                "TrayIcon" => tray_apply(&cfg),
                "CachyUpdate" | "RebootNotification" => quiet::apply(&cfg, false),
                _ => {}
            }
            0
        }
        _ => {
            bad(tr("Usage: cachy-auto-update config [list | get KEY | set KEY VALUE]"));
            1
        }
    }
}

/// Pads by characters, not bytes, so "Prüfung" lines up.
fn row(label: &str, value: &str) {
    let pad = 28usize.saturating_sub(label.chars().count());
    println!("  {label}{} {value}", " ".repeat(pad));
}

fn onoff(on: bool) -> String {
    let c = colors();
    if on { format!("{}{}{}", c.green, tr("ON"), c.reset) } else { format!("{}{}{}", c.dim, tr("OFF"), c.reset) }
}

fn status_cmd() -> i32 {
    status::init_time_locale();
    let cfg = Config::load();
    let c = colors();
    let loc = ui_locale();
    println!("\n{}{}  {PRETTY}{}\n", c.bold, c.blue, c.reset);
    let st = Status::read();
    row(tr("Automatic updates"), &onoff(cfg.enabled));
    row(tr("How often"), &status::interval_text(&loc, &cfg.interval));
    row(tr("When"), &status::hours_text(&loc, &cfg.get("UpdateHours")));
    row(tr("Notifications"), &onoff(cfg.notifications));
    row(tr("Restart automatically"), status::reboot_text(&loc, cfg.auto_reboot));
    println!();
    row(tr("Last check"), &status::time_ago(&loc, st.last_run));
    if st.last_success > 0 {
        row(
            tr("Last successful update"),
            &format!("{} ({})", status::strftime(st.last_success, "%c"), trf("%d packages", &[&st.total()])),
        );
    } else {
        row(tr("Last successful update"), tr("never"));
    }
    let next = match status::next_update(&cfg, st.last_success) {
        Next::At(ts) => status::strftime(ts, "%c"),
        Next::NotScheduled => tr("not scheduled").into(),
        Next::Unknown => tr("unknown").into(),
    };
    row(tr("Next scheduled run"), &next);
    let say = |color: &str, msg: &str| println!("\n  {color}{msg}{}", c.reset);
    match st.result {
        LastResult::Failed => say(c.yellow, tr("The last run reported a problem. See 'cachy-auto-update log'.")),
        LastResult::Interrupted => say(c.yellow, tr("The last run was stopped before it finished.")),
        LastResult::Cancelled => say(c.dim, tr("The last run was cancelled. It will run again later.")),
        _ => {}
    }
    let planned = cachy_auto_update::reboot::will_happen(&cfg, st.restart_planned);
    if st.reboot {
        say(c.yellow, tr("A restart is recommended to finish a kernel update."));
    } else if planned {
        say(c.dim, tr("A restart is planned to finish the update."));
    }
    if planned {
        println!("  {}", tr("It happens by itself once nobody is logged in."));
    }
    if !st.held.is_empty() {
        say(c.yellow, &trf("Held back: %s", &[&st.held.join(" ")]));
    }

    // Each check on its own. Without this there is no way to tell why an
    // updater that is on has done nothing on somebody else's machine.
    println!("\n  {}", tr("Current conditions:"));
    let line = |mark: String, msg: &str| println!("    {mark} {msg}");
    let good = format!("{}✔{}", c.green, c.reset);
    let note = format!("{}•{}", c.dim, c.reset);
    let warn = format!("{}•{}", c.yellow, c.reset);
    if conditions::on_ac() {
        line(good.clone(), tr("on AC power"));
    } else {
        match conditions::battery_percent() {
            Some(p) => line(note, &trf("on battery (%d%%, threshold %d%%)", &[&p, &cfg.min_battery])),
            None => line(note, tr("on battery")),
        }
    }
    match conditions::busy() {
        Some(why) => line(warn.clone(), &why),
        None => line(good.clone(), tr("no game detected")),
    }
    match locks::package_manager_busy() {
        Some(why) => line(warn, &why),
        None => line(good, tr("package system is free")),
    }
    println!();
    0
}

fn log_cmd(args: &[String]) -> i32 {
    let (mut file, mut lines, mut follow) = (paths().runlog(), "200".to_string(), false);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-n" => lines = it.next().cloned().unwrap_or(lines),
            "-f" | "--follow" => follow = true,
            "-a" | "--all" => file = paths().logfile(),
            s if s.starts_with("-n") => lines = s[2..].to_string(),
            _ => {}
        }
    }
    if !lines.chars().all(|c| c.is_ascii_digit()) || lines.is_empty() {
        lines = "200".into();
    }
    // The log folder may be root's only on older installs.
    if std::fs::File::open(&file).is_err() && paths().log.is_dir() && !proc::is_root() {
        need_root();
    }
    if std::fs::File::open(&file).is_err() {
        hint(tr("No log yet."));
        return 0;
    }
    let mut cmd = Command::new("tail");
    cmd.args(["-n", &lines]);
    if follow {
        cmd.arg("-f");
    }
    let err = cmd.arg(&file).exec();
    bad(&err.to_string());
    1
}

fn help() -> i32 {
    println!("{PRETTY} {VERSION}\n");
    println!("{}\n", tr("Usage: cachy-auto-update [command]"));
    println!("{}", tr("Commands:"));
    let cmds: &[(&str, &str)] = &[
        ("enable", "Turn automatic updates on"),
        ("disable", "Turn automatic updates off"),
        ("notifications on|off", "Turn desktop notifications on or off"),
        ("status", "Show the current state and conditions"),
        ("run [--force] [--dry-run]", "Run an update now"),
        ("log [-n N] [-f] [-a]", "Show the log of the last run"),
        ("config [get|set KEY ...]", "Show or change a setting"),
        ("-h, --help", "Show this help"),
        ("-V, --version", "Show the version"),
    ];
    for (cmd, what) in cmds {
        println!("  {cmd:<26}{}", tr(what));
    }
    println!("\n{}", tr("Without a command the window opens."));
    0
}

/// Without a command: the window, where there is a desktop; else the status
/// and the help.
fn default_cmd() -> i32 {
    let desktop = std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_some();
    if desktop && !proc::is_root() && gui_exe().exists() {
        let err = Command::new(gui_exe()).exec();
        bad(&err.to_string());
    }
    status_cmd();
    help()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (cmd, rest) = match args.split_first() {
        Some((c, r)) => (c.as_str(), r),
        None => ("", &[][..]),
    };
    // `status | head` ends quietly, as any command line tool does. Not the
    // update run: it writes to the progress helpers' pipes, and one that went
    // away must not take the update with it.
    if cmd != "run" {
        unsafe {
            libc::signal(libc::SIGPIPE, libc::SIG_DFL);
        }
    }
    let code = match cmd {
        "enable" => enable(),
        "disable" => disable(),
        "notifications" => notifications(rest.first().map(String::as_str)),
        "status" => status_cmd(),
        "run" => {
            need_root();
            runner::main(rest)
        }
        "log" => log_cmd(rest),
        "config" => config_cmd(rest),
        // From the autostart entry, as the user: messages from while nobody
        // was logged in.
        "--deliver-notifications" => {
            notify::deliver_queue();
            0
        }
        // Run by the update as a user, inside their session.
        "_notify" => notify::cli_notify(rest),
        "_notify-close" => {
            if let Some(id) = rest.first().and_then(|i| i.parse().ok()) {
                notify::desktop_close(id);
            }
            0
        }
        "_progress" => cachy_auto_update::job::main(rest),
        "-h" | "--help" | "help" => help(),
        "-V" | "--version" => {
            println!("{NAME} {VERSION}");
            0
        }
        "" => default_cmd(),
        other => {
            bad(&trf("Unknown command: %s", &[&other]));
            println!();
            help();
            1
        }
    };
    std::process::exit(code);
}
