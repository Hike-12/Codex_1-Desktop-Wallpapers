use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use core_engine::{Renderer, Result, shader::ShaderFile};
use winit::{
    application::ApplicationHandler,
    dpi::{PhysicalPosition, PhysicalSize},
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, DeviceEvents, EventLoop},
    platform::x11::{EventLoopBuilderExtX11, WindowAttributesExtX11, WindowType},
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
    window::{Window, WindowId},
};
use x11rb::{
    protocol::xproto::{AtomEnum, ConnectionExt, PropMode},
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
};

use crate::{FRAME_INTERVAL, SHADER_INTERVAL};

const COVER_CHECK_INTERVAL: Duration = Duration::from_millis(250);

x11rb::atom_manager! {
    Atoms: AtomCookies {
        _NET_SUPPORTED,
        _NET_ACTIVE_WINDOW,
        _NET_WM_DESKTOP,
        _NET_WM_STATE,
        _NET_WM_STATE_FULLSCREEN,
        _NET_WM_WINDOW_TYPE_DESKTOP,
    }
}

pub fn run(shader: Option<PathBuf>) -> Result<()> {
    let mut app = App {
        initial_shader: shader,
        wallpaper: None,
        error: None,
    };
    EventLoop::builder().with_x11().build()?.run_app(&mut app)?;
    app.error.map_or(Ok(()), Err)
}

struct App {
    initial_shader: Option<PathBuf>,
    wallpaper: Option<Wallpaper>,
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
        if self.wallpaper.is_some() {
            return;
        }
        event_loop.listen_device_events(DeviceEvents::Never);
        match Wallpaper::new(event_loop, self.initial_shader.take()) {
            Ok(wallpaper) => self.wallpaper = Some(wallpaper),
            Err(error) => self.fail(event_loop, error),
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some(wallpaper) = self
            .wallpaper
            .as_mut()
            .filter(|state| state.window.id() == id)
        else {
            return;
        };
        match event {
            WindowEvent::CloseRequested | WindowEvent::Destroyed => event_loop.exit(),
            WindowEvent::RedrawRequested if wallpaper.paused_at.is_none() => {
                let size = wallpaper.window.inner_size();
                let elapsed = wallpaper.started.elapsed() - wallpaper.paused_duration;
                if let Err(error) =
                    wallpaper
                        .renderer
                        .render(size.width, size.height, elapsed.as_secs_f32())
                {
                    self.fail(event_loop, error);
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(wallpaper) = &mut self.wallpaper {
            event_loop.set_control_flow(ControlFlow::WaitUntil(wallpaper.update()));
        }
    }
}

struct Wallpaper {
    renderer: Renderer<'static>,
    window: Arc<Window>,
    desktop: Desktop,
    shader: Option<ShaderFile>,
    started: Instant,
    paused_at: Option<Instant>,
    paused_duration: Duration,
    next_frame: Instant,
    next_cover_check: Instant,
    next_shader_check: Instant,
}

impl Wallpaper {
    fn new(event_loop: &ActiveEventLoop, path: Option<PathBuf>) -> Result<Self> {
        let monitor = event_loop
            .primary_monitor()
            .ok_or("X11 has no primary display")?;
        let bounds = Bounds {
            position: monitor.position(),
            size: monitor.size(),
        };
        let window = Arc::new(
            event_loop.create_window(
                Window::default_attributes()
                    .with_title("Desktop Wallpapers")
                    .with_visible(false)
                    .with_active(false)
                    .with_decorations(false)
                    .with_position(bounds.position)
                    .with_inner_size(bounds.size)
                    .with_x11_window_type(vec![WindowType::Desktop]),
            )?,
        );
        let desktop = Desktop::attach(&window, bounds)?;
        let renderer = pollster::block_on(Renderer::new(
            window.clone(),
            bounds.size.width,
            bounds.size.height,
        ))?;
        let shader = path.and_then(|path| match ShaderFile::resolve(path) {
            Ok(shader) => Some(shader),
            Err(error) => {
                eprintln!("wallp: {error}");
                None
            }
        });
        let now = Instant::now();
        let paused_at = desktop.covered().then_some(now);
        window.set_visible(true);
        Ok(Self {
            renderer,
            window,
            desktop,
            shader,
            started: now,
            paused_at,
            paused_duration: Duration::ZERO,
            next_frame: now,
            next_cover_check: now,
            next_shader_check: now,
        })
    }

    fn update(&mut self) -> Instant {
        let now = Instant::now();
        if now >= self.next_cover_check {
            self.next_cover_check = now + COVER_CHECK_INTERVAL;
            match (self.paused_at, self.desktop.covered()) {
                (None, true) => self.paused_at = Some(now),
                (Some(start), false) => {
                    self.paused_duration += now - start;
                    self.paused_at = None;
                }
                _ => {}
            }
        }
        if self.paused_at.is_some() {
            return self.next_cover_check;
        }
        let mut deadline = self.next_cover_check;
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
        deadline.min(self.next_frame)
    }
}

#[derive(Clone, Copy)]
struct Bounds {
    position: PhysicalPosition<i32>,
    size: PhysicalSize<u32>,
}

impl Bounds {
    fn covers(self, monitor: Self) -> bool {
        let right = |bounds: Self| i64::from(bounds.position.x) + i64::from(bounds.size.width);
        let bottom = |bounds: Self| i64::from(bounds.position.y) + i64::from(bounds.size.height);
        self.position.x <= monitor.position.x
            && self.position.y <= monitor.position.y
            && right(self) >= right(monitor)
            && bottom(self) >= bottom(monitor)
    }
}

struct Desktop {
    connection: RustConnection,
    root: u32,
    window: u32,
    atoms: Atoms,
    monitor: Bounds,
}

impl Desktop {
    fn attach(window: &Window, monitor: Bounds) -> Result<Self> {
        let window_id = match window.window_handle()?.as_raw() {
            RawWindowHandle::Xlib(handle) => u32::try_from(handle.window)?,
            RawWindowHandle::Xcb(handle) => handle.window.get(),
            _ => return Err("Expected a native X11 window".into()),
        };
        let (connection, _) = x11rb::connect(None)?;
        let root = connection.get_geometry(window_id)?.reply()?.root;
        let atoms = Atoms::new(&connection)?.reply()?;
        let supported = connection
            .get_property(false, root, atoms._NET_SUPPORTED, AtomEnum::ATOM, 0, 4096)?
            .reply()?;
        if !supported.value32().is_some_and(|mut atoms_list| {
            atoms_list.any(|atom| atom == atoms._NET_WM_WINDOW_TYPE_DESKTOP)
        }) {
            return Err("The X11 window manager does not advertise EWMH desktop windows".into());
        }

        // Set the ICCCM input hint before mapping so the wallpaper never takes keyboard focus.
        let hints = connection
            .get_property(
                false,
                window_id,
                AtomEnum::WM_HINTS,
                AtomEnum::WM_HINTS,
                0,
                9,
            )?
            .reply()?;
        let mut hints: Vec<u32> = hints.value32().into_iter().flatten().collect();
        hints.resize(9, 0);
        hints[0] |= 1;
        hints[1] = 0;
        connection
            .change_property32(
                PropMode::REPLACE,
                window_id,
                AtomEnum::WM_HINTS,
                AtomEnum::WM_HINTS,
                &hints,
            )?
            .check()?;
        connection
            .change_property32(
                PropMode::REPLACE,
                window_id,
                atoms._NET_WM_DESKTOP,
                AtomEnum::CARDINAL,
                &[u32::MAX],
            )?
            .check()?;
        window.set_cursor_hittest(false)?;
        Ok(Self {
            connection,
            root,
            window: window_id,
            atoms,
            monitor,
        })
    }

    fn covered(&self) -> bool {
        // The foreground window can disappear between queries.
        self.foreground_covers_monitor().unwrap_or(false)
    }

    fn foreground_covers_monitor(&self) -> Result<bool> {
        let active = self
            .connection
            .get_property(
                false,
                self.root,
                self.atoms._NET_ACTIVE_WINDOW,
                AtomEnum::WINDOW,
                0,
                1,
            )?
            .reply()?;
        let Some(active) = active
            .value32()
            .and_then(|mut windows| windows.next())
            .filter(|active| *active != x11rb::NONE && *active != self.window)
        else {
            return Ok(false);
        };
        let state = self
            .connection
            .get_property(
                false,
                active,
                self.atoms._NET_WM_STATE,
                AtomEnum::ATOM,
                0,
                64,
            )?
            .reply()?;
        if !state.value32().is_some_and(|mut states| {
            states.any(|state| state == self.atoms._NET_WM_STATE_FULLSCREEN)
        }) {
            return Ok(false);
        }
        let geometry = self.connection.get_geometry(active)?.reply()?;
        let coordinates = self
            .connection
            .translate_coordinates(active, self.root, 0, 0)?
            .reply()?;
        Ok(Bounds {
            position: PhysicalPosition::new(
                i32::from(coordinates.dst_x),
                i32::from(coordinates.dst_y),
            ),
            size: PhysicalSize::new(u32::from(geometry.width), u32::from(geometry.height)),
        }
        .covers(self.monitor))
    }
}
