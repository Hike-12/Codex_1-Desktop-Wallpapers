#![cfg(target_os = "macos")]

use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use core_engine::{Renderer, Result, shader::ShaderFile};
use objc2_app_kit::{NSScreen, NSView, NSWindowCollectionBehavior};
use objc2_core_graphics::{CGWindowLevelForKey, CGWindowLevelKey};
use objc2_foundation::MainThreadMarker;
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, DeviceEvents, EventLoop},
    platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS},
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
    window::{Window, WindowId},
};

const FRAME_INTERVAL: Duration = Duration::from_millis(33);
const SHADER_INTERVAL: Duration = Duration::from_millis(500);

pub fn run(shader: Option<PathBuf>) -> Result<()> {
    let event_loop = EventLoop::builder()
        .with_activation_policy(ActivationPolicy::Accessory)
        .with_activate_ignoring_other_apps(false)
        .with_default_menu(false)
        .build()?;
    let mut app = App {
        wallpaper: None,
        shader,
        error: None,
    };
    event_loop.run_app(&mut app)?;
    app.error.map_or(Ok(()), Err)
}

struct App {
    wallpaper: Option<Wallpaper>,
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
        if self.wallpaper.is_some() {
            return;
        }
        event_loop.listen_device_events(DeviceEvents::Never);
        match Wallpaper::new(event_loop, self.shader.take()) {
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
            WindowEvent::Occluded(occluded) => wallpaper.set_occluded(occluded),
            WindowEvent::RedrawRequested if wallpaper.paused_since.is_none() => {
                let size = wallpaper.window.inner_size();
                let elapsed = wallpaper.animation_start.elapsed().as_secs_f32();
                match wallpaper.renderer.render(size.width, size.height, elapsed) {
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
        if let Some(wallpaper) = &mut self.wallpaper {
            event_loop.set_control_flow(wallpaper.update());
        }
    }
}

struct Wallpaper {
    renderer: Renderer<'static>,
    window: Arc<Window>,
    shader: Option<ShaderFile>,
    animation_start: Instant,
    next_frame: Instant,
    next_shader_check: Instant,
    paused_since: Option<Instant>,
    presented: bool,
}

impl Wallpaper {
    fn new(event_loop: &ActiveEventLoop, shader: Option<PathBuf>) -> Result<Self> {
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
        place_on_desktop(&window)?;
        let size = window.inner_size();
        let renderer = pollster::block_on(Renderer::new(window.clone(), size.width, size.height))?;
        let shader = shader.and_then(|path| match ShaderFile::resolve(path) {
            Ok(shader) => Some(shader),
            Err(error) => {
                eprintln!("wallp: {error}");
                None
            }
        });
        let now = Instant::now();
        Ok(Self {
            renderer,
            window,
            shader,
            animation_start: now,
            next_frame: now,
            next_shader_check: now,
            paused_since: None,
            presented: false,
        })
    }

    fn set_occluded(&mut self, occluded: bool) {
        match (occluded, self.paused_since) {
            (true, None) => {
                self.paused_since = Some(Instant::now());
                eprintln!("wallp: paused while the wallpaper is covered");
            }
            (false, Some(start)) => {
                self.paused_since = None;
                self.animation_start += start.elapsed();
                eprintln!("wallp: resumed rendering");
            }
            _ => {}
        }
    }

    fn update(&mut self) -> ControlFlow {
        if self.paused_since.is_some() {
            return ControlFlow::Wait;
        }
        let now = Instant::now();
        if now >= self.next_frame {
            self.next_frame = now + FRAME_INTERVAL;
            self.window.request_redraw();
        }
        let mut deadline = self.next_frame;
        if let Some(shader) = &mut self.shader {
            if now >= self.next_shader_check {
                self.next_shader_check = now + SHADER_INTERVAL;
                shader.reload(&mut self.renderer);
            }
            deadline = deadline.min(self.next_shader_check);
        }
        ControlFlow::WaitUntil(deadline)
    }
}

fn place_on_desktop(window: &Window) -> Result<()> {
    let main_thread = MainThreadMarker::new().ok_or("AppKit requires the main thread")?;
    let RawWindowHandle::AppKit(handle) = window.window_handle()?.as_raw() else {
        return Err("Expected an AppKit window".into());
    };
    // Winit owns the NSView. Its window stays alive while the Window is borrowed.
    let view = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
    let window = view
        .window()
        .ok_or("The wallpaper view has no AppKit window")?;
    let screens = NSScreen::screens(main_thread);
    let screen = screens.first().ok_or("macOS reports no primary display")?;
    window.setFrame_display(screen.frame(), false);
    window.setLevel(CGWindowLevelForKey(CGWindowLevelKey::DesktopWindowLevelKey) as isize);
    window.setIgnoresMouseEvents(true);
    // The window is live on the AppKit thread and these flags use one behavior
    // per collection category (Spaces, Mission Control, and window cycling).
    unsafe {
        window.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::IgnoresCycle,
        );
        window.setCanHide(false);
        window.setHidesOnDeactivate(false);
        window.orderFrontRegardless();
    }
    Ok(())
}
