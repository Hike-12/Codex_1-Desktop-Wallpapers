use std::{path::PathBuf, ptr::NonNull, time::Instant};

use core_engine::{Renderer, Result, shader::ShaderFile};
use raw_window_handle::{
    RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle,
};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, Region, SurfaceData},
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
    globals::registry_queue_init,
    protocol::{wl_output, wl_surface},
};

use crate::{FRAME_INTERVAL, SHADER_INTERVAL};

pub fn run(path: Option<PathBuf>) -> Result<()> {
    let connection = Connection::connect_to_env()?;
    let (globals, queue) = registry_queue_init(&connection)?;
    let handle = queue.handle();
    let compositor = CompositorState::bind(&globals, &handle)?;
    let shell = LayerShell::bind(&globals, &handle)
        .map_err(|_| "This Wayland compositor does not expose zwlr_layer_shell_v1")?;
    let state = OutputState::new(&globals, &handle);
    let surface = compositor.create_surface(&handle);
    let layer = shell.create_layer_surface(
        &handle,
        surface,
        Layer::Background,
        Some("desktop-wallpapers"),
        None,
    );
    layer.set_anchor(Anchor::all());
    layer.set_size(0, 0);
    layer.set_exclusive_zone(-1);
    layer.set_keyboard_interactivity(KeyboardInteractivity::None);
    let input = Region::new(&compositor)?;
    layer.wl_surface().set_input_region(Some(input.wl_region()));
    layer.commit();
    drop(input);

    let mut event_loop = EventLoop::try_new()?;
    WaylandSource::new(connection.clone(), queue).insert(event_loop.handle())?;

    let shader = path.and_then(|path| match ShaderFile::resolve(path) {
        Ok(shader) => Some(shader),
        Err(error) => {
            eprintln!("wallp: {error}");
            None
        }
    });
    let now = Instant::now();
    let mut wallpaper = Wallpaper {
        renderer: None,
        layer,
        output_state: state,
        registry_state: RegistryState::new(&globals),
        configure: None,
        scale: 1,
        ready: true,
        closed: false,
        shader,
        started: now,
        next_frame: now,
        next_shader_check: now,
    };
    while !wallpaper.closed {
        let timeout = if wallpaper.ready && wallpaper.configure.is_some() {
            let now = Instant::now();
            if now >= wallpaper.next_frame {
                wallpaper.draw(&connection, &handle, now)?;
            }
            wallpaper.ready.then(|| {
                wallpaper
                    .next_frame
                    .saturating_duration_since(Instant::now())
            })
        } else {
            None
        };
        event_loop.dispatch(timeout, &mut wallpaper)?;
    }
    Ok(())
}

struct Wallpaper {
    // The surface borrows Wayland pointers. Drop the WGPU objects before the native layer.
    renderer: Option<Renderer<'static>>,
    layer: LayerSurface,
    output_state: OutputState,
    registry_state: RegistryState,
    configure: Option<(u32, u32)>,
    scale: u32,
    ready: bool,
    closed: bool,
    shader: Option<ShaderFile>,
    started: Instant,
    next_frame: Instant,
    next_shader_check: Instant,
}

impl Wallpaper {
    fn draw(
        &mut self,
        connection: &Connection,
        handle: &QueueHandle<Self>,
        now: Instant,
    ) -> Result<()> {
        let (width, height) = self.buffer_size()?;
        if self.renderer.is_none() {
            self.renderer = Some(create_renderer(connection, &self.layer, width, height)?);
        }
        let renderer = self.renderer.as_mut().expect("renderer is initialized");
        self.next_frame = now + FRAME_INTERVAL;
        if let Some(shader) = &mut self.shader
            && now >= self.next_shader_check
        {
            self.next_shader_check = now + SHADER_INTERVAL;
            shader.reload(renderer);
        }
        if renderer.render(width, height, self.started.elapsed().as_secs_f32())? {
            // The compositor decides when this surface is ready for another frame.
            // Request the callback after a successful present so a failed acquire can't strand it.
            let surface = self.layer.wl_surface();
            surface.frame(handle, surface.clone());
            self.layer.commit();
            self.ready = false;
        }
        Ok(())
    }

    fn buffer_size(&self) -> Result<(u32, u32)> {
        let (width, height) = self
            .configure
            .expect("the compositor configured this layer");
        if width > 0 && height > 0 {
            return Ok((
                width.saturating_mul(self.scale),
                height.saturating_mul(self.scale),
            ));
        }
        let surface = self.layer.wl_surface();
        let info = surface
            .data::<SurfaceData>()
            .and_then(|data| {
                data.outputs()
                    .find_map(|output| self.output_state.info(&output))
            })
            .or_else(|| {
                self.output_state
                    .outputs()
                    .find_map(|output| self.output_state.info(&output))
            })
            .ok_or("The Wayland compositor did not specify wallpaper dimensions")?;
        let (fallback_width, fallback_height) = info
            .logical_size
            .or_else(|| {
                let mode = info.modes.iter().find(|mode| mode.current)?;
                let scale = info.scale_factor.max(1);
                Some((mode.dimensions.0 / scale, mode.dimensions.1 / scale))
            })
            .ok_or("The Wayland compositor did not specify wallpaper dimensions")?;
        let width = if width == 0 {
            u32::try_from(fallback_width)?
        } else {
            width
        };
        let height = if height == 0 {
            u32::try_from(fallback_height)?
        } else {
            height
        };
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

impl CompositorHandler for Wallpaper {
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
        if surface.version() >= 3 && scale > 0 {
            surface.set_buffer_scale(scale);
            self.scale = scale as u32;
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

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {
        self.ready = true;
    }
}

impl LayerShellHandler for Wallpaper {
    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _: u32,
    ) {
        self.configure = Some(configure.new_size);
    }

    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {
        self.closed = true;
    }
}

impl OutputHandler for Wallpaper {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl ProvidesRegistryState for Wallpaper {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }

    registry_handlers![OutputState];
}

delegate_compositor!(Wallpaper);
delegate_layer!(Wallpaper);
delegate_output!(Wallpaper);
delegate_registry!(Wallpaper);
