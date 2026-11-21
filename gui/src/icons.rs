//! Line icons on a 24 point grid, drawn with the text colour.

use rune::Icon;

macro_rules! line_icon {
    ($static:ident, $name:literal, [$($d:literal),+ $(,)?]) => {
        pub static $static: Icon = Icon {
            name: $name,
            view: 24.0,
            body: concat!($(
                "<path d=\"", $d,
                "\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.75\" stroke-linecap=\"round\" stroke-linejoin=\"round\"/>"
            ),+),
        };
    };
}

line_icon!(HOME, "cau-home", [
    "M15 21v-8a1 1 0 0 0-1-1h-4a1 1 0 0 0-1 1v8",
    "M3 10a2 2 0 0 1 .709-1.528l7-5.999a2 2 0 0 1 2.582 0l7 5.999A2 2 0 0 1 21 10v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z",
]);

line_icon!(CLOCK, "cau-clock", [
    "M12 2a10 10 0 1 0 0 20a10 10 0 1 0 0-20z",
    "M12 6v6l4 2",
]);

line_icon!(PACKAGE, "cau-package", [
    "M11 21.73a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73z",
    "M12 22V12",
    "M3.3 7l7.703 4.734a2 2 0 0 0 1.994 0L20.7 7",
    "M7.5 4.27l9 5.15",
]);

line_icon!(BELL, "cau-bell", [
    "M10.268 21a2 2 0 0 0 3.464 0",
    "M3.262 15.326A1 1 0 0 0 4 17h16a1 1 0 0 0 .74-1.673C19.41 13.956 18 12.499 18 8A6 6 0 0 0 6 8c0 4.499-1.411 5.956-2.738 7.326",
]);

line_icon!(BROOM, "cau-broom", [
    "M3 6h18",
    "M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6",
    "M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2",
    "M10 11v6",
    "M14 11v6",
]);

line_icon!(LOG, "cau-log", [
    "M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7z",
    "M14 2v4a2 2 0 0 0 2 2h4",
    "M16 13H8",
    "M16 17H8",
    "M10 9H8",
]);

line_icon!(CHECK_CIRCLE, "cau-check-circle", [
    "M21.801 10A10 10 0 1 1 17 3.335",
    "M9 11l3 3L22 4",
]);

line_icon!(DOWNLOAD, "cau-download", [
    "M12 15V3",
    "M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4",
    "M7 10l5 5 5-5",
]);

line_icon!(REFRESH, "cau-refresh", [
    "M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8",
    "M21 3v5h-5",
    "M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16",
    "M8 16H3v5",
]);

line_icon!(WARNING, "cau-warning", [
    "M21.73 18l-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3",
    "M12 9v4",
    "M12 17h.01",
]);

line_icon!(PLUG, "cau-plug", [
    "M12 22v-5",
    "M9 8V2",
    "M15 8V2",
    "M18 8v5a4 4 0 0 1-4 4h-4a4 4 0 0 1-4-4V8z",
]);

line_icon!(BATTERY, "cau-battery", [
    "M4 7h12a2 2 0 0 1 2 2v6a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V9a2 2 0 0 1 2-2z",
    "M22 11v2",
    "M6 11v2",
]);

line_icon!(GAMEPAD, "cau-gamepad", [
    "M6 11h4",
    "M8 9v4",
    "M15 12h.01",
    "M18 10h.01",
    "M17.32 5H6.68a4 4 0 0 0-3.978 3.59C2.604 9.416 2 14.456 2 16a3 3 0 0 0 3 3c1 0 1.5-.5 2-1l1.414-1.414A2 2 0 0 1 9.828 16h4.344a2 2 0 0 1 1.414.586L17 18c.5.5 1 1 2 1a3 3 0 0 0 3-3c0-1.545-.604-6.584-.685-7.258A4 4 0 0 0 17.32 5z",
]);

line_icon!(LOCK, "cau-lock", [
    "M5 11h14a2 2 0 0 1 2 2v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-7a2 2 0 0 1 2-2z",
    "M7 11V7a5 5 0 0 1 10 0v4",
]);

line_icon!(RESTART, "cau-restart", [
    "M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8",
    "M3 3v5h5",
]);

line_icon!(POWER, "cau-power", [
    "M12 2v10",
    "M18.4 6.6a9 9 0 1 1-12.77.04",
]);

line_icon!(PAUSE, "cau-pause", [
    "M8 5v14",
    "M16 5v14",
]);

line_icon!(INFO, "cau-info", [
    "M12 2a10 10 0 1 0 0 20a10 10 0 1 0 0-20z",
    "M12 16v-4",
    "M12 8h.01",
]);
