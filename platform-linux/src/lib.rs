#![cfg(target_os = "linux")]

use std::{path::PathBuf, time::Duration};

use core_engine::Result;

mod wayland;
mod x11;

const FRAME_INTERVAL: Duration = Duration::from_millis(33);
const SHADER_INTERVAL: Duration = Duration::from_millis(500);

pub fn run(shader: Option<PathBuf>) -> Result<()> {
    if ["WAYLAND_DISPLAY", "WAYLAND_SOCKET"]
        .iter()
        .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
        || std::env::var_os("XDG_SESSION_TYPE").is_some_and(|kind| kind == "wayland")
    {
        wayland::run(shader)
    } else if std::env::var_os("DISPLAY").is_some_and(|display| !display.is_empty()) {
        x11::run(shader)
    } else {
        Err("wallp needs a Wayland or X11 desktop session".into())
    }
}
