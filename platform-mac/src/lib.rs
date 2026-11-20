#![cfg(target_os = "macos")]

mod audio;
mod screen;

use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use core_engine::{FrameInputs, Renderer, Result, shader::SceneFile};
use objc2_app_kit::NSEvent;
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, DeviceEvents, EventLoop},
    platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS},
    window::{Window, WindowId},
};

use crate::screen::Display;

const FRAME_INTERVAL: Duration = Duration::from_millis(33);
const SHADER_INTERVAL: Duration = Duration::from_millis(500);
const DESKTOP_INTERVAL: Duration = Duration::from_secs(1);

pub fn run(shader: Option<PathBuf>, audio: bool, battery_saver: bool) -> Result<()> {
    let shader = shader.and_then(|path| match SceneFile::resolve(path) {
        Ok(shader) => Some(shader),
        Err(error) => {
            eprintln!("wallp: {error}");
            None
        }
    });
    let event_loop = EventLoop::builder()
        .with_activation_policy(ActivationPolicy::Accessory)
        .with_activate_ignoring_other_apps(false)
        .with_default_menu(false)
        .build()?;
    let now = Instant::now();
    let mut app = App {
        wallpapers: Vec::new(),
        shader,
        capture: None,
        audio_requested: audio,
        battery_saver,
        on_battery: false,
        initialized: false,
        next_desktop_check: now,
        next_shader_check: now,
        error: None,
    };
    event_loop.run_app(&mut app)?;
    app.error.map_or(Ok(()), Err)
}

struct App {
    wallpapers: Vec<Wallpaper>,
    shader: Option<SceneFile>,
    capture: Option<audio::Capture>,
    audio_requested: bool,
    battery_saver: bool,
    on_battery: bool,
    initialized: bool,
    next_desktop_check: Instant,
    next_shader_check: Instant,
    error: Option<Box<dyn std::error::Error>>,
}

impl App {
    fn fail(&mut self, event_loop: &ActiveEventLoop, error: Box<dyn std::error::Error>) {
        self.error = Some(error);
        event_loop.exit();
    }

    fn refresh_desktop(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        let displays = screen::displays()?;
        if !self.initialized && displays.is_empty() {
            return Err("macOS reports no connected displays".into());
        }
        self.wallpapers.retain(|wallpaper| {
            displays
                .iter()
                .any(|display| display.id == wallpaper.display.id)
        });
        for display in displays {
            if let Some(wallpaper) = self
                .wallpapers
                .iter_mut()
                .find(|state| state.display.id == display.id)
            {
                if wallpaper.display.frame != display.frame {
                    screen::move_window(&wallpaper.window, display)?;
                    wallpaper.display = display;
                }
            } else {
                self.wallpapers
                    .push(Wallpaper::new(event_loop, display, self.shader.as_ref())?);
            }
        }
        let on_battery = self.battery_saver && screen::on_battery();
        if on_battery != self.on_battery {
            self.on_battery = on_battery;
            let state = if on_battery {
                "paused on battery power"
            } else {
                "AC power detected"
            };
            eprintln!("wallp: {state}");
        }
        self.next_desktop_check = Instant::now() + DESKTOP_INTERVAL;
        Ok(())
    }

    fn update(&mut self, event_loop: &ActiveEventLoop) -> Result<ControlFlow> {
        if Instant::now() >= self.next_desktop_check {
            self.refresh_desktop(event_loop)?;
        }
        let now = Instant::now();
        let mut deadline = self.next_desktop_check;
        for wallpaper in &mut self.wallpapers {
            wallpaper.update_pause(self.on_battery, now);
            if wallpaper.paused_since.is_none() {
                if now >= wallpaper.next_frame {
                    wallpaper.next_frame = now + FRAME_INTERVAL;
                    wallpaper.window.request_redraw();
                }
                deadline = deadline.min(wallpaper.next_frame);
            }
        }

        let visible = self
            .wallpapers
            .iter()
            .any(|wallpaper| wallpaper.paused_since.is_none());
        self.update_capture(visible && !self.shader.as_ref().is_some_and(SceneFile::has_video));
        if let Some(scene) = &self.shader {
            scene.set_active(visible);
        }
        if visible && let Some(shader) = &mut self.shader {
            if now >= self.next_shader_check {
                self.next_shader_check = now + SHADER_INTERVAL;
                let mut renderers: Vec<_> = self
                    .wallpapers
                    .iter_mut()
                    .map(|state| &mut state.renderer)
                    .collect();
                shader.reload_all(&mut renderers);
            }
            deadline = deadline.min(self.next_shader_check);
        }
        Ok(ControlFlow::WaitUntil(deadline))
    }

    fn update_capture(&mut self, visible: bool) {
        if visible && self.audio_requested && self.capture.is_none() {
            match audio::Capture::new() {
                Ok(capture) => self.capture = Some(capture),
                Err(error) => {
                    self.audio_requested = false;
                    eprintln!("wallp: system audio is unavailable: {error}");
                }
            }
        }
        if let Some(capture) = &mut self.capture
            && let Err(error) = capture.set_running(visible)
        {
            self.capture = None;
            self.audio_requested = false;
            eprintln!("wallp: system audio stopped: {error}");
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.initialized {
            return;
        }
        event_loop.listen_device_events(DeviceEvents::Never);
        match self.refresh_desktop(event_loop) {
            Ok(()) => self.initialized = true,
            Err(error) => self.fail(event_loop, error),
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some(index) = self
            .wallpapers
            .iter()
            .position(|state| state.window.id() == id)
        else {
            return;
        };
        let wallpaper = &mut self.wallpapers[index];
        match event {
            WindowEvent::CloseRequested | WindowEvent::Destroyed => event_loop.exit(),
            WindowEvent::Occluded(occluded) => {
                wallpaper.occluded = occluded;
                wallpaper.update_pause(self.on_battery, Instant::now());
            }
            WindowEvent::RedrawRequested
                if wallpaper.paused_since.is_none() && !self.on_battery =>
            {
                let size = wallpaper.window.inner_size();
                let elapsed = wallpaper.animation_start.elapsed().as_secs_f32();
                // AppKit dispatches window events on its main thread.
                let point = unsafe { NSEvent::mouseLocation() };
                let inputs = FrameInputs {
                    cursor: wallpaper.display.cursor(point),
                    audio_bands: self
                        .capture
                        .as_mut()
                        .map_or([0.0; 16], audio::Capture::bands),
                };
                match wallpaper.renderer.render_with_inputs(
                    size.width,
                    size.height,
                    elapsed,
                    &inputs,
                ) {
                    Ok(true) if !wallpaper.presented => {
                        wallpaper.presented = true;
                        eprintln!(
                            "wallp: wallpaper is rendering on display {}",
                            wallpaper.display.id
                        );
                    }
                    Ok(_) => {}
                    Err(error) => self.fail(event_loop, error),
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.initialized {
            match self.update(event_loop) {
                Ok(flow) => event_loop.set_control_flow(flow),
                Err(error) => self.fail(event_loop, error),
            }
        }
    }
}

struct Wallpaper {
    renderer: Renderer<'static>,
    window: Arc<Window>,
    display: Display,
    animation_start: Instant,
    next_frame: Instant,
    paused_since: Option<Instant>,
    occluded: bool,
    presented: bool,
}

impl Wallpaper {
    fn new(
        event_loop: &ActiveEventLoop,
        display: Display,
        shader: Option<&SceneFile>,
    ) -> Result<Self> {
        let window = Arc::new(
            event_loop.create_window(
                Window::default_attributes()
                    .with_title("Desktop Wallpapers")
                    .with_visible(false)
                    .with_decorations(false)
                    .with_resizable(false)
                    .with_active(false),
            )?,
        );
        screen::place_window(&window, display)?;
        let size = window.inner_size();
        let mut renderer =
            pollster::block_on(Renderer::new(window.clone(), size.width, size.height))?;
        if let Some(shader) = shader {
            shader.apply_to(&mut renderer)?;
        }
        let now = Instant::now();
        Ok(Self {
            renderer,
            window,
            display,
            animation_start: now,
            next_frame: now,
            paused_since: None,
            occluded: false,
            presented: false,
        })
    }

    fn update_pause(&mut self, on_battery: bool, now: Instant) {
        match (self.occluded || on_battery, self.paused_since) {
            (true, None) => {
                self.paused_since = Some(now);
                eprintln!("wallp: paused display {}", self.display.id);
            }
            (false, Some(start)) => {
                self.paused_since = None;
                self.animation_start += now - start;
                self.next_frame = now;
                eprintln!("wallp: resumed display {}", self.display.id);
            }
            _ => {}
        }
    }
}
