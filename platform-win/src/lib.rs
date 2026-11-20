#![cfg(windows)]

mod app;
mod audio;
mod desktop;
mod icons;
mod ipc;
mod tray;

pub use app::run;
