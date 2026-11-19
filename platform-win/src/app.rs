use std::{
    ffi::OsStr,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use core_engine::{DEFAULT_SHADER, Renderer, Result};
use tokio::sync::oneshot;
use tray_icon::menu::MenuEvent;
use windows_sys::Win32::Foundation::RECT;
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, DeviceEvents, EventLoop},
    window::{Window, WindowId},
};

use crate::{
    desktop::{Desktop, primary_bounds},
    ipc,
    shader::ShaderFile,
    tray::{self, Action, Tray},
};

const FRAME_INTERVAL: Duration = Duration::from_millis(33);
const DESKTOP_INTERVAL: Duration = Duration::from_millis(250);
const SHADER_INTERVAL: Duration = Duration::from_millis(500);

const USAGE: &str =
    "Usage: wallp [shader.wgsl | pack-folder]\n       wallp set <shader.wgsl | pack-folder>";

pub fn run() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let shader = args.next();
    if matches!(
        shader.as_deref().and_then(OsStr::to_str),
        Some("--help" | "-h")
    ) {
        println!(
            "{USAGE}\n\nRun on the primary display. Use the system tray or 'wallp set' to switch while running.\n\
             Packs in %APPDATA%\\wallp\\wallpapers appear in the tray. Each needs a manifest.toml\n\
             with a name and a relative shader path, for example shader = \"shader.wgsl\"."
        );
        return Ok(());
    }
    if shader.as_deref() == Some(OsStr::new("set")) {
        let path = args.next().ok_or(USAGE)?;
        if args.next().is_some() {
            return Err(USAGE.into());
        }
        ipc::set(PathBuf::from(path).as_path())?;
        println!("Wallpaper updated.");
        return Ok(());
    }
    if args.next().is_some() {
        return Err(USAGE.into());
    }
    let event_loop = EventLoop::<Event>::with_user_event().build()?;
    ipc::listen(event_loop.create_proxy())?;
    let proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |event| {
        let _ = proxy.send_event(Event::Menu(event));
    }));
    let mut app = App {
        running: None,
        shader: shader.map(PathBuf::from),
        error: None,
        tray: None,
    };
    event_loop.run_app(&mut app)?;
    app.error.map_or(Ok(()), Err)
}

pub enum Event {
    Set {
        path: PathBuf,
        reply: oneshot::Sender<std::result::Result<(), String>>,
    },
    Menu(MenuEvent),
}

struct App {
    running: Option<Wallpaper>,
    shader: Option<PathBuf>,
    error: Option<Box<dyn std::error::Error>>,
    tray: Option<Tray>,
}

impl App {
    fn fail(&mut self, event_loop: &ActiveEventLoop, error: Box<dyn std::error::Error>) {
        self.error = Some(error);
        event_loop.exit();
    }

    fn menu_action(&mut self, event_loop: &ActiveEventLoop, event: MenuEvent) -> Result<()> {
        let (Some(tray), Some(wallpaper)) = (&mut self.tray, &mut self.running) else {
            return Ok(());
        };
        match tray.action(&event.id)? {
            Some(Action::BuiltIn) => wallpaper.select_builtin()?,
            Some(Action::Select(path)) => wallpaper.select_shader(path)?,
            Some(Action::Quit) => event_loop.exit(),
            None => {}
        }
        Ok(())
    }
}

impl ApplicationHandler<Event> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.running.is_some() {
            return;
        }
        event_loop.listen_device_events(DeviceEvents::Never);
        match Wallpaper::new(event_loop, self.shader.take()) {
            Ok(wallpaper) => {
                self.running = Some(wallpaper);
                match Tray::new() {
                    Ok(tray) => self.tray = Some(tray),
                    Err(error) => eprintln!("wallp: could not create the tray icon: {error}"),
                }
            }
            Err(error) => self.fail(event_loop, error),
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: Event) {
        match event {
            Event::Set { path, reply } => {
                if reply.is_closed() {
                    return;
                }
                let result = self
                    .running
                    .as_mut()
                    .ok_or_else(|| "wallp is still starting".to_owned())
                    .and_then(|wallpaper| {
                        wallpaper
                            .select_shader(path)
                            .map_err(|error| error.to_string())
                    });
                let _ = reply.send(result);
            }
            Event::Menu(event) => {
                if let Err(error) = self.menu_action(event_loop, event) {
                    let message = error.to_string();
                    eprintln!("wallp: {message}");
                    tray::show_error(&message);
                }
            }
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

    fn exiting(&mut self, _: &ActiveEventLoop) {
        self.tray = None;
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
    shader: Option<ShaderFile>,
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
        let shader = shader.and_then(|path| match ShaderFile::resolve(path) {
            Ok(shader) => Some(shader),
            Err(error) => {
                eprintln!("wallp: {error}");
                None
            }
        });
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
            shader,
        })
    }

    fn select_shader(&mut self, path: PathBuf) -> Result<()> {
        let shader = ShaderFile::resolve(path)?.select(&mut self.renderer)?;
        self.shader = Some(shader);
        self.next_shader_check = Instant::now() + SHADER_INTERVAL;
        Ok(())
    }

    fn select_builtin(&mut self) -> Result<()> {
        if self.shader.is_some() {
            pollster::block_on(self.renderer.set_shader(DEFAULT_SHADER))?;
            self.shader = None;
            eprintln!("wallp: loaded the built-in wallpaper");
        }
        Ok(())
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
