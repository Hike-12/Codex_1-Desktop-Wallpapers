#![cfg(windows)]

mod app;
mod audio;
mod desktop;
mod ipc;
mod tray;

pub use app::run;
