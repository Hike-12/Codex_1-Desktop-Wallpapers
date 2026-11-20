use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use core_engine::{FrameInputs, Renderer, Result, shader::SceneFile};
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
    connection::Connection,
    protocol::xproto::{AtomEnum, ConnectionExt, MapState, PropMode},
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
};

use crate::{
    FRAME_INTERVAL, FrameClock, SHADER_INTERVAL, audio::Playback, initial_shader,
    power::BatterySaver,
};

const COVER_CHECK_INTERVAL: Duration = Duration::from_millis(250);

x11rb::atom_manager! {
    Atoms: AtomCookies {
        _NET_SUPPORTED,
        _NET_CLIENT_LIST_STACKING,
        _NET_CURRENT_DESKTOP,
        _NET_WM_DESKTOP,
        _NET_WM_STATE,
        _NET_WM_STATE_FULLSCREEN,
        _NET_WM_STATE_HIDDEN,
        _NET_WM_WINDOW_TYPE_DESKTOP,
    }
}

pub fn run(path: Option<PathBuf>, audio: bool, battery_saver: bool) -> Result<()> {
    let now = Instant::now();
    let mut app = App {
        desktop: Desktop::connect()?,
        wallpapers: Vec::new(),
        covers: Vec::new(),
        audio: Playback::start(audio),
        battery: BatterySaver::new(battery_saver),
        pointer: None,
        shader: initial_shader(path),
        next_cover_check: now,
        next_shader_check: now,
        error: None,
    };
    EventLoop::builder().with_x11().build()?.run_app(&mut app)?;
    app.error.map_or(Ok(()), Err)
}

struct App {
    desktop: Desktop,
    wallpapers: Vec<Wallpaper>,
    covers: Vec<Bounds>,
    audio: Option<Playback>,
    battery: BatterySaver,
    pointer: Option<PhysicalPosition<i32>>,
    shader: Option<SceneFile>,
    next_cover_check: Instant,
    next_shader_check: Instant,
    error: Option<Box<dyn std::error::Error>>,
}

impl App {
    fn fail(&mut self, event_loop: &ActiveEventLoop, error: Box<dyn std::error::Error>) {
        self.error = Some(error);
        event_loop.exit();
    }

    fn sync_monitors(&mut self, event_loop: &ActiveEventLoop, now: Instant) -> Result<()> {
        let bounds: Vec<_> = event_loop
            .available_monitors()
            .map(|monitor| Bounds {
                position: monitor.position(),
                size: monitor.size(),
            })
            .collect();
        self.wallpapers
            .retain(|wallpaper| bounds.contains(&wallpaper.bounds));
        for bounds in bounds {
            if self
                .wallpapers
                .iter()
                .any(|wallpaper| wallpaper.bounds == bounds)
            {
                continue;
            }
            let mut wallpaper = Wallpaper::new(event_loop, &self.desktop, bounds, now)?;
            if let Some(shader) = &self.shader
                && let Err(error) = shader.apply_to(&mut wallpaper.renderer)
            {
                eprintln!("wallp: could not apply shader on the new X11 monitor: {error}");
            }
            self.wallpapers.push(wallpaper);
        }
        Ok(())
    }

    fn update(&mut self, event_loop: &ActiveEventLoop) -> Result<Instant> {
        let now = Instant::now();
        if now >= self.next_cover_check {
            self.next_cover_check = now + COVER_CHECK_INTERVAL;
            self.sync_monitors(event_loop, now)?;
            self.covers = self.desktop.fullscreen_bounds()?;
        }
        let battery = self.battery.update(now);
        let mut deadline = self
            .battery
            .deadline()
            .unwrap_or(self.next_cover_check)
            .min(self.next_cover_check);
        for wallpaper in &mut self.wallpapers {
            let covered = self
                .covers
                .iter()
                .any(|bounds| bounds.covers(wallpaper.bounds));
            wallpaper.clock.set_paused(battery || covered, now);
        }
        let active = self
            .wallpapers
            .iter()
            .any(|wallpaper| wallpaper.clock.paused_at.is_none());
        if let Some(audio) = &self.audio {
            audio.set_active(active && !self.shader.as_ref().is_some_and(SceneFile::has_video));
        }
        if let Some(scene) = &self.shader {
            scene.set_active(active);
        }
        if !active {
            return Ok(deadline);
        }
        if let Some(shader) = &mut self.shader {
            if now >= self.next_shader_check {
                self.next_shader_check = now + SHADER_INTERVAL;
                let mut renderers: Vec<_> = self
                    .wallpapers
                    .iter_mut()
                    .map(|wallpaper| &mut wallpaper.renderer)
                    .collect();
                shader.reload_all(&mut renderers);
            }
            deadline = deadline.min(self.next_shader_check);
        }
        if self.wallpapers.iter().any(|wallpaper| {
            wallpaper.clock.paused_at.is_none() && now >= wallpaper.clock.next_frame
        }) {
            self.pointer = self.desktop.pointer();
        }
        for wallpaper in &mut self.wallpapers {
            if wallpaper.clock.paused_at.is_some() {
                continue;
            }
            if now >= wallpaper.clock.next_frame {
                wallpaper.clock.next_frame = now + FRAME_INTERVAL;
                wallpaper.window.request_redraw();
            }
            deadline = deadline.min(wallpaper.clock.next_frame);
        }
        Ok(deadline)
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        event_loop.listen_device_events(DeviceEvents::Never);
        self.next_cover_check = Instant::now();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some(wallpaper) = self
            .wallpapers
            .iter_mut()
            .find(|state| state.window.id() == id)
        else {
            return;
        };
        match event {
            WindowEvent::CloseRequested | WindowEvent::Destroyed => event_loop.exit(),
            WindowEvent::RedrawRequested if wallpaper.clock.paused_at.is_none() => {
                let size = wallpaper.window.inner_size();
                let inputs = FrameInputs {
                    cursor: self
                        .pointer
                        .and_then(|pointer| wallpaper.bounds.normalize(pointer)),
                    audio_bands: self.audio.as_ref().map_or([0.0; 16], Playback::bands),
                };
                if let Err(error) = wallpaper.renderer.render_with_inputs(
                    size.width,
                    size.height,
                    wallpaper.clock.animation_time(Instant::now()),
                    &inputs,
                ) {
                    self.fail(event_loop, error);
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        match self.update(event_loop) {
            Ok(deadline) => event_loop.set_control_flow(ControlFlow::WaitUntil(deadline)),
            Err(error) => self.fail(event_loop, error),
        }
    }
}

struct Wallpaper {
    renderer: Renderer<'static>,
    window: Arc<Window>,
    bounds: Bounds,
    clock: FrameClock,
}

impl Wallpaper {
    fn new(
        event_loop: &ActiveEventLoop,
        desktop: &Desktop,
        bounds: Bounds,
        now: Instant,
    ) -> Result<Self> {
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
        desktop.attach(&window)?;
        let renderer = pollster::block_on(Renderer::new(
            window.clone(),
            bounds.size.width,
            bounds.size.height,
        ))?;
        window.set_visible(true);
        Ok(Self {
            renderer,
            window,
            bounds,
            clock: FrameClock::new(now),
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
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

    fn normalize(self, point: PhysicalPosition<i32>) -> Option<[f32; 2]> {
        let x = i64::from(point.x) - i64::from(self.position.x);
        let y = i64::from(point.y) - i64::from(self.position.y);
        if !(0..i64::from(self.size.width)).contains(&x)
            || !(0..i64::from(self.size.height)).contains(&y)
        {
            return None;
        }
        Some([
            x as f32 / self.size.width as f32,
            y as f32 / self.size.height as f32,
        ])
    }
}

struct Desktop {
    connection: RustConnection,
    root: u32,
    atoms: Atoms,
}

impl Desktop {
    fn connect() -> Result<Self> {
        let (connection, screen) = x11rb::connect(None)?;
        let root = connection.setup().roots[screen].root;
        let atoms = Atoms::new(&connection)?.reply()?;
        let supported = connection
            .get_property(false, root, atoms._NET_SUPPORTED, AtomEnum::ATOM, 0, 4096)?
            .reply()?;
        if !supported
            .value32()
            .is_some_and(|mut list| list.any(|atom| atom == atoms._NET_WM_WINDOW_TYPE_DESKTOP))
        {
            return Err("The X11 window manager does not advertise EWMH desktop windows".into());
        }
        Ok(Self {
            connection,
            root,
            atoms,
        })
    }

    fn attach(&self, window: &Window) -> Result<()> {
        let window_id = match window.window_handle()?.as_raw() {
            RawWindowHandle::Xlib(handle) => u32::try_from(handle.window)?,
            RawWindowHandle::Xcb(handle) => handle.window.get(),
            _ => return Err("Expected a native X11 window".into()),
        };
        let hints = self
            .connection
            .get_property(
                false,
                window_id,
                AtomEnum::WM_HINTS,
                AtomEnum::WM_HINTS,
                0,
                9,
            )?
            .reply()?;
        // Clear the ICCCM input hint before mapping the passive window.
        let mut hints: Vec<u32> = hints.value32().into_iter().flatten().collect();
        hints.resize(9, 0);
        hints[0] |= 1;
        hints[1] = 0;
        self.connection
            .change_property32(
                PropMode::REPLACE,
                window_id,
                AtomEnum::WM_HINTS,
                AtomEnum::WM_HINTS,
                &hints,
            )?
            .check()?;
        self.connection
            .change_property32(
                PropMode::REPLACE,
                window_id,
                self.atoms._NET_WM_DESKTOP,
                AtomEnum::CARDINAL,
                &[u32::MAX],
            )?
            .check()?;
        window.set_cursor_hittest(false)?;
        Ok(())
    }

    fn pointer(&self) -> Option<PhysicalPosition<i32>> {
        let pointer = self
            .connection
            .query_pointer(self.root)
            .ok()?
            .reply()
            .ok()?;
        pointer
            .same_screen
            .then(|| PhysicalPosition::new(i32::from(pointer.root_x), i32::from(pointer.root_y)))
    }

    fn fullscreen_bounds(&self) -> Result<Vec<Bounds>> {
        let desktop = self.desktop_number(self.root, self.atoms._NET_CURRENT_DESKTOP);
        let clients = self
            .connection
            .get_property(
                false,
                self.root,
                self.atoms._NET_CLIENT_LIST_STACKING,
                AtomEnum::WINDOW,
                0,
                u32::MAX,
            )?
            .reply()?;
        // Queue state queries before waiting for replies. Clients can vanish during the scan.
        let states: Vec<_> = clients
            .value32()
            .into_iter()
            .flatten()
            .filter_map(|window| {
                let query = self
                    .connection
                    .get_property(
                        false,
                        window,
                        self.atoms._NET_WM_STATE,
                        AtomEnum::ATOM,
                        0,
                        64,
                    )
                    .ok()?;
                Some((window, query))
            })
            .collect();
        let mut bounds = Vec::new();
        for (window, state) in states {
            let Ok(state) = state.reply() else { continue };
            let Some(atoms) = state.value32() else {
                continue;
            };
            let (fullscreen, hidden) = atoms.fold((false, false), |(fullscreen, hidden), atom| {
                (
                    fullscreen || atom == self.atoms._NET_WM_STATE_FULLSCREEN,
                    hidden || atom == self.atoms._NET_WM_STATE_HIDDEN,
                )
            });
            if !fullscreen || hidden {
                continue;
            }
            if let Some(rect) = self.visible_geometry(window, desktop) {
                bounds.push(rect);
            }
        }
        Ok(bounds)
    }

    fn desktop_number(&self, window: u32, property: u32) -> Option<u32> {
        self.connection
            .get_property(false, window, property, AtomEnum::CARDINAL, 0, 1)
            .ok()?
            .reply()
            .ok()?
            .value32()?
            .next()
    }

    fn visible_geometry(&self, window: u32, desktop: Option<u32>) -> Option<Bounds> {
        if let (Some(current), Some(window_desktop)) = (
            desktop,
            self.desktop_number(window, self.atoms._NET_WM_DESKTOP),
        ) && window_desktop != current
            && window_desktop != u32::MAX
        {
            return None;
        }
        let attributes = self
            .connection
            .get_window_attributes(window)
            .ok()?
            .reply()
            .ok()?;
        if attributes.map_state != MapState::VIEWABLE {
            return None;
        }
        let geometry = self.connection.get_geometry(window).ok()?.reply().ok()?;
        let coordinates = self
            .connection
            .translate_coordinates(window, self.root, 0, 0)
            .ok()?
            .reply()
            .ok()?;
        Some(Bounds {
            position: PhysicalPosition::new(
                i32::from(coordinates.dst_x),
                i32::from(coordinates.dst_y),
            ),
            size: PhysicalSize::new(u32::from(geometry.width), u32::from(geometry.height)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monitor_bounds_scope_cursor_and_fullscreen() {
        let left = Bounds {
            position: PhysicalPosition::new(-800, 0),
            size: PhysicalSize::new(800, 600),
        };
        let right = Bounds {
            position: PhysicalPosition::new(0, 0),
            size: PhysicalSize::new(800, 600),
        };
        assert!(left.covers(left));
        assert!(!left.covers(right));
        assert_eq!(
            left.normalize(PhysicalPosition::new(-400, 300)),
            Some([0.5, 0.5])
        );
        assert_eq!(left.normalize(PhysicalPosition::new(0, 300)), None);
        assert_eq!(
            right.normalize(PhysicalPosition::new(0, 300)),
            Some([0.0, 0.5])
        );
    }
}
