#![cfg(windows)]

mod app;
mod audio;
mod desktop;
mod gallery;
mod icons;
mod ipc;
mod tray;

pub use app::run;
pub use tray::show_error;
