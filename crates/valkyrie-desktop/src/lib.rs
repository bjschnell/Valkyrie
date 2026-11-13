//! The desktop app's shell (ADR-0020): a native window around the web app that
//! `valk web` serves on this machine. Everything a user sees inside the window is
//! the web app; this crate is what the window stands on.
//!
//! - `start`: make sure `valk web` answers on the local address, set it up for
//!   this machine if not, and check that what answers is the user's own.
//! - `install`: `valk` itself, for someone who installed only the app.
//! - `pair`: the one pairing link a launch may mint, vouched by the daemon.
//! - `watch`: the daemon client behind the tray's count and native notifications.
//!
//! None of it needs a webview. The window, the tray icon and starting at login
//! wrap these and are not here yet.

pub mod install;
pub mod pair;
pub mod start;
pub mod watch;
