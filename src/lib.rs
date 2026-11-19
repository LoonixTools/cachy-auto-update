//! cachy-auto-update: unattended updates for CachyOS.
//!
//! The library holds everything the command line, the update run, the window
//! and the tray icon share.

pub mod check;
pub mod conditions;
pub mod config;
pub mod control;
pub mod i18n;
pub mod job;
pub mod locks;
pub mod log;
pub mod notify;
pub mod paths;
pub mod pkg;
pub mod proc;
pub mod progress;
pub mod quiet;
pub mod reboot;
pub mod runner;
pub mod state;
pub mod status;
pub mod users;
