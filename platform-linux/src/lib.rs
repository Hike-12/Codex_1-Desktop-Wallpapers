#![cfg(target_os = "linux")]

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use core_engine::{Result, shader::ShaderFile};

mod audio;
mod power;
mod wayland;
mod x11;

const FRAME_INTERVAL: Duration = Duration::from_millis(33);
const SHADER_INTERVAL: Duration = Duration::from_millis(500);

pub fn run(shader: Option<PathBuf>, audio: bool, battery_saver: bool) -> Result<()> {
    if ["WAYLAND_DISPLAY", "WAYLAND_SOCKET"]
        .iter()
        .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
        || std::env::var_os("XDG_SESSION_TYPE").is_some_and(|kind| kind == "wayland")
    {
        wayland::run(shader, audio, battery_saver)
    } else if std::env::var_os("DISPLAY").is_some_and(|display| !display.is_empty()) {
        x11::run(shader, audio, battery_saver)
    } else {
        Err("wallp needs a Wayland or X11 desktop session".into())
    }
}

fn initial_shader(path: Option<PathBuf>) -> Option<ShaderFile> {
    path.and_then(|path| match ShaderFile::resolve(path) {
        Ok(shader) => Some(shader),
        Err(error) => {
            eprintln!("wallp: {error}");
            None
        }
    })
}

struct FrameClock {
    start: Instant,
    next_frame: Instant,
    paused_at: Option<Instant>,
    paused_time: Duration,
}

impl FrameClock {
    fn new(now: Instant) -> Self {
        Self {
            start: now,
            next_frame: now,
            paused_at: None,
            paused_time: Duration::ZERO,
        }
    }

    fn set_paused(&mut self, paused: bool, now: Instant) {
        match (self.paused_at, paused) {
            (None, true) => self.paused_at = Some(now),
            (Some(start), false) => {
                self.paused_time += now - start;
                self.paused_at = None;
            }
            _ => {}
        }
    }

    fn animation_time(&self, now: Instant) -> f32 {
        (now - self.start - self.paused_time).as_secs_f32()
    }
}
