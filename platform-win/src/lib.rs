#![cfg(windows)]

mod app;
mod audio;
mod desktop;
mod gallery;
mod icons;
mod ipc;
mod tray;

const BRAND_ICON: &[u8] = include_bytes!("../../branding/stilltide.png");

pub use app::run;
pub use tray::show_error;
