use std::{
    ffi::OsStr,
    fs,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use core_engine::{Renderer, Result};
use windows_sys::Win32::Foundation::RECT;
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, DeviceEvents, EventLoop},
    window::{Window, WindowId},
};

use crate::desktop::{Desktop, primary_bounds};

const FRAME_INTERVAL: Duration = Duration::from_millis(33);
const DESKTOP_INTERVAL: Duration = Duration::from_millis(250);
const SHADER_INTERVAL: Duration = Duration::from_millis(500);

pub fn run() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let shader = args.next();
    if shader.as_deref() == Some(OsStr::new("--help")) {
        println!(
            "Usage: wallp [shader.wgsl]\nRun the wallpaper on the primary display. Press Ctrl+C to stop."
        );
        return Ok(());
    }
    if args.next().is_some() {
        return Err("Usage: wallp [shader.wgsl]".into());
    }
    let mut app = App {
        running: None,
        shader: shader.map(PathBuf::from),
        error: None,
    };
    EventLoop::new()?.run_app(&mut app)?;
    app.error.map_or(Ok(()), Err)
}

struct App {
    running: Option<Wallpaper>,
    shader: Option<PathBuf>,
    error: Option<Box<dyn std::error::Error>>,
}

impl App {
    fn fail(&mut self, event_loop: &ActiveEventLoop, error: Box<dyn std::error::Error>) {
        self.error = Some(error);
        event_loop.exit();
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.running.is_some() {
            return;
        }
        event_loop.listen_device_events(DeviceEvents::Never);
        match Wallpaper::new(event_loop, self.shader.take()) {
            Ok(wallpaper) => self.running = Some(wallpaper),
            Err(error) => self.fail(event_loop, error),
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some(wallpaper) = self
            .running
            .as_mut()
            .filter(|state| state.window.id() == id)
        else {
            return;
        };
        match event {
            WindowEvent::CloseRequested | WindowEvent::Destroyed => event_loop.exit(),
            WindowEvent::RedrawRequested if !wallpaper.paused => {
                let size = wallpaper.window.inner_size();
                let time = (wallpaper.started.elapsed() - wallpaper.paused_duration).as_secs_f32();
                match wallpaper.renderer.render(size.width, size.height, time) {
                    Ok(true) if !wallpaper.presented => {
                        wallpaper.presented = true;
                        eprintln!("wallp: wallpaper is rendering");
                    }
                    Ok(_) => {}
                    Err(error) => self.fail(event_loop, error),
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(wallpaper) = &mut self.running {
            match wallpaper.update() {
                Ok(deadline) => event_loop.set_control_flow(ControlFlow::WaitUntil(deadline)),
                Err(error) => self.fail(event_loop, error),
            }
        }
    }
}

struct Wallpaper {
    renderer: Renderer<'static>,
    window: Arc<Window>,
    desktop: Desktop,
    monitor: RECT,
    started: Instant,
    next_frame: Instant,
    next_desktop_check: Instant,
    next_shader_check: Instant,
    paused: bool,
    paused_at: Instant,
    paused_duration: Duration,
    presented: bool,
    shader: Option<LocalShader>,
}

impl Wallpaper {
    fn new(event_loop: &ActiveEventLoop, shader: Option<PathBuf>) -> Result<Self> {
        let window = Arc::new(
            event_loop.create_window(
                Window::default_attributes()
                    .with_title("Desktop Wallpapers")
                    .with_visible(false)
                    .with_decorations(false)
                    .with_resizable(false),
            )?,
        );
        let desktop = Desktop::attach(&window)?;
        let size = window.inner_size();
        let renderer = pollster::block_on(Renderer::new(window.clone(), size.width, size.height))?;
        let now = Instant::now();
        let monitor = primary_bounds();
        let paused = desktop.covered(monitor);
        if paused {
            eprintln!("wallp: paused for fullscreen");
        }
        Ok(Self {
            renderer,
            window,
            desktop,
            monitor,
            started: now,
            next_frame: now,
            next_desktop_check: now,
            next_shader_check: now,
            paused,
            paused_at: now,
            paused_duration: Duration::ZERO,
            presented: false,
            shader: shader.map(|path| LocalShader {
                path,
                last_read: None,
            }),
        })
    }

    fn update(&mut self) -> Result<Instant> {
        let now = Instant::now();
        if now >= self.next_desktop_check {
            self.next_desktop_check = now + DESKTOP_INTERVAL;
            if !self.desktop.is_attached() {
                return Err("Explorer's wallpaper window was closed".into());
            }
            let monitor = primary_bounds();
            if (monitor.right, monitor.bottom) != (self.monitor.right, self.monitor.bottom) {
                self.desktop.fit(monitor)?;
                self.monitor = monitor;
            }
            let covered = self.desktop.covered(self.monitor);
            if covered != self.paused {
                self.paused = covered;
                if covered {
                    self.paused_at = now;
                    eprintln!("wallp: paused for fullscreen");
                } else {
                    self.paused_duration += now - self.paused_at;
                    eprintln!("wallp: resumed rendering");
                }
            }
        }
        if self.paused {
            return Ok(self.next_desktop_check);
        }

        let mut deadline = self.next_desktop_check;
        if let Some(shader) = &mut self.shader {
            if now >= self.next_shader_check {
                self.next_shader_check = now + SHADER_INTERVAL;
                shader.reload(&mut self.renderer);
            }
            deadline = deadline.min(self.next_shader_check);
        }
        if now >= self.next_frame {
            self.next_frame = now + FRAME_INTERVAL;
            self.window.request_redraw();
        }
        Ok(deadline.min(self.next_frame))
    }
}

struct LocalShader {
    path: PathBuf,
    last_read: Option<std::result::Result<String, String>>,
}

impl LocalShader {
    fn reload(&mut self, renderer: &mut Renderer<'_>) {
        let read = fs::read_to_string(&self.path).map_err(|error| error.to_string());
        if self.last_read.as_ref() == Some(&read) {
            return;
        }
        match &read {
            Ok(source) => match pollster::block_on(renderer.set_shader(source)) {
                Ok(()) => eprintln!("wallp: loaded {}", self.path.display()),
                Err(error) => eprintln!("wallp: {}: {error}", self.path.display()),
            },
            Err(error) => eprintln!("wallp: {}: {error}", self.path.display()),
        }
        self.last_read = Some(read);
    }
}
