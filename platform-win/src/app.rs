use std::{
    ffi::OsStr,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use core_engine::{
    DEFAULT_SHADER, FrameInputs, Renderer, Result,
    shader::{SceneFile, set_shader_all},
};
use tokio::sync::oneshot;
use tray_icon::menu::MenuEvent;
use windows_sys::Win32::Foundation::RECT;
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, DeviceEvents, EventLoop},
    platform::windows::MonitorHandleExtWindows,
    window::{Window, WindowId},
};

use crate::{
    audio::PlaybackAudio,
    desktop::{self, Desktop, DesktopLayer},
    icons, ipc,
    tray::{self, Action, Tray},
};

const FRAME_INTERVAL: Duration = Duration::from_millis(33);
const DESKTOP_INTERVAL: Duration = Duration::from_millis(250);
const SHADER_INTERVAL: Duration = Duration::from_millis(500);

const USAGE: &str = "Usage: wallp [--audio] [--battery-saver] [shader.wgsl | clip.mp4 | pack-folder]\n       wallp set <shader.wgsl | clip.mp4 | pack-folder>";

pub fn run() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let first = args.next();
    if first.as_deref() == Some(OsStr::new("set")) {
        let path = args.next().ok_or(USAGE)?;
        if args.next().is_some() {
            return Err(USAGE.into());
        }
        ipc::set(PathBuf::from(path).as_path())?;
        println!("Wallpaper updated.");
        return Ok(());
    }
    let mut shader = None;
    let mut battery_saver = false;
    let mut audio = false;
    let mut parsing_options = true;
    for argument in first.into_iter().chain(args) {
        let option = if parsing_options {
            argument.to_str()
        } else {
            None
        };
        match option {
            Some("--help" | "-h") => {
                println!(
                    "{USAGE}\n\nRun on desktop displays. Use the system tray or 'wallp set' to switch while running.\n\
                     --audio reads system playback audio for the shader.\n\
                     --battery-saver pauses wallpapers while running on battery.\n\
                     Packs in %APPDATA%\\wallp\\wallpapers appear in the tray. Each needs a manifest.toml\n\
                     with a name and either a relative shader or video path. MP4 needs FFmpeg on PATH."
                );
                return Ok(());
            }
            Some("--audio") => audio = true,
            Some("--battery-saver") => battery_saver = true,
            Some("--") => parsing_options = false,
            Some(flag) if flag.starts_with('-') => {
                return Err(format!("Unknown option {flag}\n{USAGE}").into());
            }
            _ if shader.is_none() => shader = Some(PathBuf::from(argument)),
            _ => return Err(USAGE.into()),
        }
    }
    let event_loop = EventLoop::<Event>::with_user_event().build()?;
    ipc::listen(event_loop.create_proxy())?;
    let proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |event| {
        let _ = proxy.send_event(Event::Menu(event));
    }));
    let mut app = App {
        running: None,
        shader,
        audio,
        battery_saver,
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
    running: Option<Wallpapers>,
    shader: Option<PathBuf>,
    audio: bool,
    battery_saver: bool,
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
            Some(Action::Select(path)) => wallpaper.select_scene(path)?,
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
        match Wallpapers::new(
            event_loop,
            self.shader.take(),
            self.audio,
            self.battery_saver,
        ) {
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
                            .select_scene(path)
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
        let Some(running) = self.running.as_mut() else {
            return;
        };
        let Some(wallpaper) = running
            .surfaces
            .iter_mut()
            .find(|wallpaper| wallpaper.window.id() == id)
        else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested if wallpaper.paused_at.is_none() => {
                let audio_bands = running
                    .audio
                    .as_ref()
                    .map_or([0.0; 16], PlaybackAudio::bands);
                if let Err(error) = wallpaper.draw(audio_bands) {
                    self.fail(event_loop, error);
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(wallpaper) = &mut self.running {
            match wallpaper.update(event_loop) {
                Ok(deadline) => event_loop.set_control_flow(ControlFlow::WaitUntil(deadline)),
                Err(error) => self.fail(event_loop, error),
            }
        }
    }

    fn exiting(&mut self, _: &ActiveEventLoop) {
        self.tray = None;
    }
}

struct Wallpapers {
    surfaces: Vec<Wallpaper>,
    layer: DesktopLayer,
    shader: Option<SceneFile>,
    audio: Option<PlaybackAudio>,
    battery_saver: bool,
    next_desktop_check: Instant,
    next_shader_check: Instant,
    icons: Option<icons::Reader>,
    icon_snapshot: Option<icons::Snapshot>,
}

impl Wallpapers {
    fn new(
        event_loop: &ActiveEventLoop,
        shader: Option<PathBuf>,
        audio: bool,
        battery_saver: bool,
    ) -> Result<Self> {
        let shader = shader.and_then(|path| match SceneFile::resolve(path) {
            Ok(shader) => Some(shader),
            Err(error) => {
                eprintln!("wallp: {error}");
                None
            }
        });
        let audio = if audio {
            match PlaybackAudio::new() {
                Ok(audio) => Some(audio),
                Err(error) => {
                    eprintln!("wallp: could not start playback audio: {error}");
                    None
                }
            }
        } else {
            None
        };
        let now = Instant::now();
        let mut wallpapers = Self {
            surfaces: Vec::new(),
            layer: DesktopLayer::find()?,
            shader,
            audio,
            battery_saver,
            next_desktop_check: now,
            next_shader_check: now,
            icons: None,
            icon_snapshot: None,
        };
        wallpapers.refresh_desktop(event_loop, now)?;
        Ok(wallpapers)
    }

    fn select_scene(&mut self, path: PathBuf) -> Result<()> {
        if self.surfaces.is_empty() {
            return Err("No desktop display is available".into());
        }
        let mut renderers = self
            .surfaces
            .iter_mut()
            .map(|wallpaper| &mut wallpaper.renderer)
            .collect::<Vec<_>>();
        let shader = SceneFile::resolve(path)?.select_all(&mut renderers)?;
        self.shader = Some(shader);
        self.next_desktop_check = Instant::now();
        self.next_shader_check = Instant::now() + SHADER_INTERVAL;
        Ok(())
    }

    fn select_builtin(&mut self) -> Result<()> {
        if self.shader.is_some() {
            let mut renderers = self
                .surfaces
                .iter_mut()
                .map(|wallpaper| &mut wallpaper.renderer)
                .collect::<Vec<_>>();
            set_shader_all(DEFAULT_SHADER, &mut renderers)?;
            self.shader = None;
            eprintln!("wallp: loaded the built-in wallpaper");
        }
        Ok(())
    }

    fn refresh_desktop(&mut self, event_loop: &ActiveEventLoop, now: Instant) -> Result<()> {
        let monitors: Vec<_> = event_loop
            .available_monitors()
            .filter_map(|monitor| {
                let position = monitor.position();
                let size = monitor.size();
                (size.width > 0 && size.height > 0).then_some((
                    monitor.hmonitor(),
                    RECT {
                        left: position.x,
                        top: position.y,
                        right: position.x + size.width as i32,
                        bottom: position.y + size.height as i32,
                    },
                ))
            })
            .collect();
        self.surfaces
            .retain(|wallpaper| monitors.iter().any(|(id, _)| *id == wallpaper.monitor));
        let on_battery = self.battery_saver && desktop::on_battery();
        let covered = if on_battery {
            vec![false; monitors.len()]
        } else {
            desktop::covered_displays(
                &monitors
                    .iter()
                    .map(|(_, bounds)| *bounds)
                    .collect::<Vec<_>>(),
            )
        };
        for ((id, bounds), fullscreen) in monitors.into_iter().zip(covered) {
            let wallpaper = if let Some(index) = self
                .surfaces
                .iter()
                .position(|wallpaper| wallpaper.monitor == id)
            {
                &mut self.surfaces[index]
            } else {
                let mut wallpaper = Wallpaper::new(event_loop, self.layer, id, bounds)?;
                if let Some(shader) = &self.shader {
                    shader.apply_to(&mut wallpaper.renderer)?;
                }
                self.surfaces.push(wallpaper);
                self.surfaces.last_mut().expect("wallpaper was just added")
            };
            if !wallpaper.desktop.is_attached() {
                return Err("Explorer's wallpaper window was closed".into());
            }
            if bounds_key(wallpaper.bounds) != bounds_key(bounds) {
                wallpaper.desktop.fit(bounds)?;
                wallpaper.bounds = bounds;
            }
            let reason = if on_battery {
                Some("battery saver")
            } else if fullscreen {
                Some("fullscreen")
            } else {
                None
            };
            wallpaper.update_pause(reason, now);
        }
        self.next_desktop_check = now + DESKTOP_INTERVAL;
        Ok(())
    }

    fn update(&mut self, event_loop: &ActiveEventLoop) -> Result<Instant> {
        let now = Instant::now();
        let desktop_due = now >= self.next_desktop_check;
        if desktop_due {
            self.refresh_desktop(event_loop, now)?;
        }
        let mut deadline = self.next_desktop_check;
        let active = self
            .surfaces
            .iter()
            .any(|wallpaper| wallpaper.paused_at.is_none());
        if let Some(audio) = &self.audio {
            audio.set_active(active && !self.shader.as_ref().is_some_and(SceneFile::has_video));
        }
        if let Some(scene) = &self.shader {
            scene.set_active(active);
        }
        let scene_due = active && now >= self.next_shader_check;
        if active && let Some(shader) = &mut self.shader {
            if now >= self.next_shader_check {
                self.next_shader_check = now + SHADER_INTERVAL;
                let mut renderers = self
                    .surfaces
                    .iter_mut()
                    .map(|wallpaper| &mut wallpaper.renderer)
                    .collect::<Vec<_>>();
                shader.reload_all(&mut renderers);
            }
            deadline = deadline.min(self.next_shader_check);
        }
        if !self
            .shader
            .as_ref()
            .is_some_and(SceneFile::uses_desktop_icons)
        {
            self.icons = None;
            self.icon_snapshot = None;
        } else if active && (desktop_due || scene_due) {
            if self.icons.is_none() {
                self.icons = Some(icons::Reader::start()?);
            }
            if let Some(snapshot) = self.icons.as_ref().and_then(icons::Reader::poll) {
                self.icon_snapshot = Some(snapshot);
            }
            if let Some(snapshot) = &self.icon_snapshot {
                for wallpaper in &mut self.surfaces {
                    wallpaper
                        .renderer
                        .update_desktop_icons(&snapshot.for_monitor(wallpaper.bounds));
                }
            }
        }
        for wallpaper in self
            .surfaces
            .iter_mut()
            .filter(|wallpaper| wallpaper.paused_at.is_none())
        {
            if now >= wallpaper.next_frame {
                wallpaper.next_frame = now + FRAME_INTERVAL;
                wallpaper.window.request_redraw();
            }
            deadline = deadline.min(wallpaper.next_frame);
        }
        Ok(deadline)
    }
}

struct Wallpaper {
    renderer: Renderer<'static>,
    window: Arc<Window>,
    desktop: Desktop,
    monitor: isize,
    bounds: RECT,
    started: Instant,
    next_frame: Instant,
    paused_at: Option<Instant>,
    paused_duration: Duration,
    presented: bool,
}

impl Wallpaper {
    fn new(
        event_loop: &ActiveEventLoop,
        layer: DesktopLayer,
        monitor: isize,
        bounds: RECT,
    ) -> Result<Self> {
        let window = Arc::new(
            event_loop.create_window(
                Window::default_attributes()
                    .with_title("Desktop Wallpapers")
                    .with_visible(false)
                    .with_decorations(false)
                    .with_resizable(false),
            )?,
        );
        let desktop = Desktop::attach(&window, layer, bounds)?;
        let size = window.inner_size();
        let renderer = pollster::block_on(Renderer::new(window.clone(), size.width, size.height))?;
        let now = Instant::now();
        Ok(Self {
            renderer,
            window,
            desktop,
            monitor,
            bounds,
            started: now,
            next_frame: now,
            paused_at: None,
            paused_duration: Duration::ZERO,
            presented: false,
        })
    }

    fn update_pause(&mut self, reason: Option<&str>, now: Instant) {
        let now = now.max(self.started);
        let (left, top, _, _) = bounds_key(self.bounds);
        match (reason, self.paused_at) {
            (Some(reason), None) => {
                self.paused_at = Some(now);
                eprintln!("wallp: paused for {reason} on display ({left}, {top})");
            }
            (None, Some(start)) => {
                self.paused_at = None;
                self.paused_duration += now - start;
                eprintln!("wallp: resumed rendering on display ({left}, {top})");
            }
            _ => {}
        }
    }

    fn draw(&mut self, audio_bands: [f32; 16]) -> Result<()> {
        let size = self.window.inner_size();
        let time = (self.started.elapsed() - self.paused_duration).as_secs_f32();
        let inputs = FrameInputs {
            cursor: desktop::cursor_in(self.bounds),
            audio_bands,
        };
        if self
            .renderer
            .render_with_inputs(size.width, size.height, time, &inputs)?
            && !self.presented
        {
            self.presented = true;
            eprintln!("wallp: wallpaper is rendering");
        }
        Ok(())
    }
}

fn bounds_key(bounds: RECT) -> (i32, i32, i32, i32) {
    (bounds.left, bounds.top, bounds.right, bounds.bottom)
}
