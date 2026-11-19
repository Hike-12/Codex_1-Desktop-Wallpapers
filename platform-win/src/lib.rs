#![cfg(windows)]

mod app;
mod desktop;
mod ipc;
mod tray;

pub use app::run;
