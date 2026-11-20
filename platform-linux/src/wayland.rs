use std::{
    collections::HashMap,
    path::PathBuf,
    ptr::NonNull,
    time::{Duration, Instant},
};

use core_engine::{FrameInputs, Renderer, Result, shader::SceneFile};
use raw_window_handle::{
    RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle,
};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, Region},
    delegate_compositor, delegate_layer, delegate_output, delegate_registry,
    output::{OutputHandler, OutputState},
    reexports::{calloop::EventLoop, calloop_wayland_source::WaylandSource},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::{
        WaylandSurface,
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
    },
};
use wayland_client::{
    Connection, Proxy, QueueHandle,
    backend::ObjectId,
    globals::registry_queue_init,
    protocol::{wl_output, wl_surface},
};
use wayland_protocols_wlr::foreign_toplevel::v1::client::zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1;

use crate::{
    FRAME_INTERVAL, FrameClock, SHADER_INTERVAL, audio::Playback, initial_shader,
    power::BatterySaver,
};

mod fullscreen;

const FRAME_CALLBACK_TIMEOUT: Duration = Duration::from_millis(500);

pub fn run(path: Option<PathBuf>, audio: bool, battery_saver: bool) -> Result<()> {
    let connection = Connection::connect_to_env()?;
    let (globals, mut queue) = registry_queue_init(&connection)?;
    let handle = queue.handle();
    let compositor = CompositorState::bind(&globals, &handle)?;
    let shell = LayerShell::bind(&globals, &handle)
        .map_err(|_| "This Wayland compositor does not expose zwlr_layer_shell_v1")?;
    let output_state = OutputState::new(&globals, &handle);
    if globals
        .bind::<ZwlrForeignToplevelManagerV1, _, _>(&handle, 2..=3, ())
        .is_err()
    {
        eprintln!(
            "wallp: Wayland fullscreen pause is limited to compositor frame pacing on this session"
        );
    }
    let mut desktop = Desktop {
        wallpapers: Vec::new(),
        compositor,
        shell,
        output_state,
        registry_state: RegistryState::new(&globals),
        toplevels: HashMap::new(),
        sway: std::env::var_os("SWAYSOCK").is_some_and(|socket| !socket.is_empty()),
        audio: Playback::start(audio),
        battery: BatterySaver::new(battery_saver),
        shader: initial_shader(path),
        next_shader_check: Instant::now(),
    };
    queue.roundtrip(&mut desktop)?;
    let outputs: Vec<_> = desktop.output_state.outputs().collect();
    for output in outputs {
        desktop.add_output(&handle, output)?;
    }
    let mut event_loop = EventLoop::try_new()?;
    WaylandSource::new(connection.clone(), queue).insert(event_loop.handle())?;
    loop {
        let timeout = desktop.update(&connection, &handle)?;
        event_loop.dispatch(timeout, &mut desktop)?;
    }
}

struct Desktop {
    wallpapers: Vec<Wallpaper>,
    compositor: CompositorState,
    shell: LayerShell,
    output_state: OutputState,
    registry_state: RegistryState,
    toplevels: HashMap<ObjectId, fullscreen::Toplevel>,
    sway: bool,
    audio: Option<Playback>,
    battery: BatterySaver,
    shader: Option<SceneFile>,
    next_shader_check: Instant,
}

impl Desktop {
    fn add_output(
        &mut self,
        handle: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) -> Result<()> {
        if self
            .wallpapers
            .iter()
            .any(|wallpaper| wallpaper.output == output)
        {
            return Ok(());
        }
        let surface = self.compositor.create_surface(handle);
        let layer = self.shell.create_layer_surface(
            handle,
            surface,
            Layer::Background,
            Some("desktop-wallpapers"),
            Some(&output),
        );
        layer.set_anchor(Anchor::all());
        layer.set_size(0, 0);
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        let input = Region::new(&self.compositor)?;
        layer.wl_surface().set_input_region(Some(input.wl_region()));
        layer.commit();
        self.wallpapers.push(Wallpaper {
            renderer: None,
            layer,
            output,
            configure: None,
            scale: 1,
            ready: true,
            visibility_probe: false,
            visible_under_stale_cover: false,
            clock: FrameClock::new(Instant::now()),
        });
        Ok(())
    }

    fn wallpaper(&mut self, surface: &wl_surface::WlSurface) -> Option<&mut Wallpaper> {
        self.wallpapers
            .iter_mut()
            .find(|wallpaper| wallpaper.layer.wl_surface() == surface)
    }

    fn update(
        &mut self,
        connection: &Connection,
        handle: &QueueHandle<Self>,
    ) -> Result<Option<Duration>> {
        let now = Instant::now();
        let battery = self.battery.update(now);
        for wallpaper in &mut self.wallpapers {
            let covered = self
                .toplevels
                .values()
                .any(|toplevel| toplevel.covers(&wallpaper.output));
            let mut paused = covered;
            if self.sway {
                let active_cover = self
                    .toplevels
                    .values()
                    .any(|toplevel| toplevel.actively_covers(&wallpaper.output));
                if !covered || active_cover {
                    wallpaper.visible_under_stale_cover = false;
                } else if !battery {
                    // A layer started under an inactive fullscreen window needs
                    // its first buffer before visibility callbacks can wake it.
                    paused = wallpaper.renderer.is_some() && !wallpaper.visible_under_stale_cover;
                    wallpaper.probe_visibility(handle);
                }
            }
            wallpaper.clock.set_paused(battery || paused, now);
            if wallpaper.is_due(now) && wallpaper.renderer.is_none() {
                let (width, height) = wallpaper.buffer_size(&self.output_state)?;
                let mut renderer = create_renderer(connection, &wallpaper.layer, width, height)?;
                if let Some(shader) = &self.shader
                    && let Err(error) = shader.apply_to(&mut renderer)
                {
                    eprintln!("wallp: could not apply shader on the new Wayland output: {error}");
                }
                wallpaper.renderer = Some(renderer);
            }
        }
        let active = self
            .wallpapers
            .iter()
            .any(|wallpaper| wallpaper.configure.is_some() && wallpaper.clock.paused_at.is_none());
        if let Some(audio) = &self.audio {
            audio.set_active(active && !self.shader.as_ref().is_some_and(SceneFile::has_video));
        }
        if let Some(scene) = &self.shader {
            scene.set_active(self.wallpapers.iter().any(|wallpaper| {
                wallpaper.configure.is_some()
                    && wallpaper.clock.paused_at.is_none()
                    && (wallpaper.ready
                        || now < wallpaper.clock.next_frame + FRAME_CALLBACK_TIMEOUT)
            }));
        }
        let drawing = self
            .wallpapers
            .iter()
            .any(|wallpaper| wallpaper.is_due(now));
        if drawing {
            if let Some(shader) = &mut self.shader
                && now >= self.next_shader_check
            {
                self.next_shader_check = now + SHADER_INTERVAL;
                let mut renderers: Vec<_> = self
                    .wallpapers
                    .iter_mut()
                    .filter_map(|wallpaper| wallpaper.renderer.as_mut())
                    .collect();
                shader.reload_all(&mut renderers);
            }
            let inputs = FrameInputs {
                // This surface is click-through. Wayland never sends it global pointer motion.
                cursor: None,
                audio_bands: self.audio.as_ref().map_or([0.0; 16], Playback::bands),
            };
            for wallpaper in &mut self.wallpapers {
                if wallpaper.is_due(now) {
                    wallpaper.draw(&self.output_state, handle, now, &inputs)?;
                }
            }
        }
        let next = self
            .wallpapers
            .iter()
            .filter(|wallpaper| wallpaper.can_draw())
            .map(|wallpaper| wallpaper.clock.next_frame)
            .chain(self.battery.deadline())
            .chain(self.video_deadline(now))
            .min();
        Ok(next.map(|deadline| deadline.saturating_duration_since(Instant::now())))
    }

    fn video_deadline(&self, now: Instant) -> Option<Instant> {
        if !self.shader.as_ref().is_some_and(SceneFile::has_video) {
            return None;
        }
        // Some compositors only report occlusion by withholding frame callbacks.
        self.wallpapers
            .iter()
            .filter(|wallpaper| !wallpaper.ready && wallpaper.clock.paused_at.is_none())
            .map(|wallpaper| wallpaper.clock.next_frame + FRAME_CALLBACK_TIMEOUT)
            .filter(|deadline| *deadline > now)
            .min()
    }
}

struct Wallpaper {
    // Drop WGPU before the native layer whose address it uses.
    renderer: Option<Renderer<'static>>,
    layer: LayerSurface,
    output: wl_output::WlOutput,
    configure: Option<(u32, u32)>,
    scale: u32,
    ready: bool,
    visibility_probe: bool,
    visible_under_stale_cover: bool,
    clock: FrameClock,
}

impl Wallpaper {
    fn probe_visibility(&mut self, handle: &QueueHandle<Desktop>) {
        // Sway can retain the toplevel's output association for hidden workspaces.
        // A fresh callback lets the visible layer resume without submitting a probe frame.
        if self.ready && self.renderer.is_some() && !self.visible_under_stale_cover {
            let surface = self.layer.wl_surface();
            surface.frame(handle, surface.clone());
            self.layer.commit();
            self.visibility_probe = true;
            self.ready = false;
        }
    }

    fn can_draw(&self) -> bool {
        self.ready && self.configure.is_some() && self.clock.paused_at.is_none()
    }

    fn is_due(&self, now: Instant) -> bool {
        self.can_draw() && now >= self.clock.next_frame
    }

    fn draw(
        &mut self,
        output_state: &OutputState,
        handle: &QueueHandle<Desktop>,
        now: Instant,
        inputs: &FrameInputs,
    ) -> Result<()> {
        let (width, height) = self.buffer_size(output_state)?;
        self.clock.next_frame = now + FRAME_INTERVAL;
        let renderer = self.renderer.as_mut().expect("renderer is initialized");
        if renderer.render_with_inputs(width, height, self.clock.animation_time(now), inputs)? {
            // Request the callback after presenting so a failed acquire can't strand it.
            let surface = self.layer.wl_surface();
            surface.frame(handle, surface.clone());
            self.layer.commit();
            self.ready = false;
        }
        Ok(())
    }

    fn buffer_size(&self, output_state: &OutputState) -> Result<(u32, u32)> {
        let (mut width, mut height) = self
            .configure
            .expect("the compositor configured this layer");
        if width == 0 || height == 0 {
            let info = output_state
                .info(&self.output)
                .ok_or("The Wayland compositor did not specify wallpaper dimensions")?;
            let (fallback_width, fallback_height) = info
                .logical_size
                .or_else(|| {
                    let mode = info.modes.iter().find(|mode| mode.current)?;
                    let scale = info.scale_factor.max(1);
                    Some((mode.dimensions.0 / scale, mode.dimensions.1 / scale))
                })
                .ok_or("The Wayland compositor did not specify wallpaper dimensions")?;
            if width == 0 {
                width = u32::try_from(fallback_width)?;
            }
            if height == 0 {
                height = u32::try_from(fallback_height)?;
            }
        }
        if width == 0 || height == 0 {
            return Err("The Wayland compositor returned an empty wallpaper size".into());
        }
        Ok((
            width.saturating_mul(self.scale),
            height.saturating_mul(self.scale),
        ))
    }
}

fn create_renderer(
    connection: &Connection,
    layer: &LayerSurface,
    width: u32,
    height: u32,
) -> Result<Renderer<'static>> {
    let display = NonNull::new(connection.backend().display_ptr().cast())
        .ok_or("The Wayland display is unavailable")?;
    let window = NonNull::new(layer.wl_surface().id().as_ptr().cast())
        .ok_or("The Wayland wallpaper surface is unavailable")?;
    let instance = wgpu::Instance::default();
    // Both native objects stay alive until the renderer is dropped.
    let surface = unsafe {
        instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
            raw_display_handle: RawDisplayHandle::Wayland(WaylandDisplayHandle::new(display)),
            raw_window_handle: RawWindowHandle::Wayland(WaylandWindowHandle::new(window)),
        })?
    };
    pollster::block_on(Renderer::from_surface(&instance, surface, width, height))
}

impl CompositorHandler for Desktop {
    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }

    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        scale: i32,
    ) {
        if surface.version() >= 3
            && scale > 0
            && let Some(wallpaper) = self.wallpaper(surface)
        {
            surface.set_buffer_scale(scale);
            wallpaper.scale = scale as u32;
        }
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        _: u32,
    ) {
        if let Some(wallpaper) = self.wallpaper(surface) {
            if std::mem::take(&mut wallpaper.visibility_probe) {
                wallpaper.visible_under_stale_cover = true;
            }
            wallpaper.ready = true;
        }
    }
}

impl LayerShellHandler for Desktop {
    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _: u32,
    ) {
        if let Some(wallpaper) = self.wallpaper(layer.wl_surface()) {
            wallpaper.configure = Some(configure.new_size);
        }
    }

    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        self.wallpapers
            .retain(|wallpaper| wallpaper.layer != *layer);
    }
}

impl OutputHandler for Desktop {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(
        &mut self,
        _: &Connection,
        handle: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        if let Err(error) = self.add_output(handle, output) {
            eprintln!("wallp: could not attach Wayland wallpaper: {error}");
        }
    }

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn output_destroyed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        self.wallpapers
            .retain(|wallpaper| wallpaper.output != output);
    }
}

impl ProvidesRegistryState for Desktop {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }

    registry_handlers![OutputState];
}

delegate_compositor!(Desktop);
delegate_layer!(Desktop);
delegate_output!(Desktop);
delegate_registry!(Desktop);
