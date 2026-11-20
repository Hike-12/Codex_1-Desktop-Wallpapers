use std::{error::Error, sync::Arc};

pub mod audio;
pub mod icons;
pub mod shader;
mod texture;
mod video;

pub type Result<T> = std::result::Result<T, Box<dyn Error>>;

pub const DEFAULT_SHADER: &str = include_str!("default.wgsl");

const UNIFORM_BYTES: u64 = 96;

#[derive(Default)]
pub struct FrameInputs {
    /// Monitor-local coordinates with the origin at the top left.
    pub cursor: Option<[f32; 2]>,
    /// Normalized playback levels, ordered from bass to treble.
    pub audio_bands: [f32; audio::BAND_COUNT],
}

pub struct Renderer<'window> {
    surface: wgpu::Surface<'window>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    shader: PreparedShader,
    layout: wgpu::PipelineLayout,
    bind_layout: wgpu::BindGroupLayout,
    globals: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    video: Option<VideoPainter>,
}

struct PreparedShader {
    pipeline: wgpu::RenderPipeline,
    texture: Option<texture::SceneTexture>,
}

impl<'window> Renderer<'window> {
    pub async fn new(
        target: impl Into<wgpu::SurfaceTarget<'window>>,
        width: u32,
        height: u32,
    ) -> Result<Self> {
        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(target)?;
        Self::from_surface(&instance, surface, width, height).await
    }

    pub async fn from_surface(
        instance: &wgpu::Instance,
        surface: wgpu::Surface<'window>,
        width: u32,
        height: u32,
    ) -> Result<Self> {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .ok_or("No GPU adapter supports the wallpaper surface")?;
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("Wallpaper"),
                    required_limits: wgpu::Limits::downlevel_defaults()
                        .using_resolution(adapter.limits()),
                    ..Default::default()
                },
                None,
            )
            .await?;
        let mut config = surface
            .get_default_config(&adapter, width.max(1), height.max(1))
            .ok_or("The GPU exposes no wallpaper surface formats")?;
        if let Some(format) = surface
            .get_capabilities(&adapter)
            .formats
            .into_iter()
            .find(wgpu::TextureFormat::is_srgb)
        {
            config.format = format;
        }
        surface.configure(&device, &config);

        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Wallpaper uniforms"),
            size: UNIFORM_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(UNIFORM_BYTES),
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &bind_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&bind_layout],
            push_constant_ranges: &[],
        });
        let pipeline = compile(&device, &layout, config.format, DEFAULT_SHADER).await?;

        Ok(Self {
            surface,
            device,
            queue,
            config,
            shader: PreparedShader {
                pipeline,
                texture: None,
            },
            layout,
            bind_layout,
            globals,
            bind_group,
            video: None,
        })
    }

    pub async fn set_shader(&mut self, source: &str) -> Result<()> {
        self.shader = self.prepare_shader(source, None).await?;
        self.video = None;
        Ok(())
    }

    pub fn update_desktop_icons(&mut self, layout: &icons::IconLayout) {
        if let Some(texture) = &mut self.shader.texture
            && self.video.is_none()
        {
            texture.update_icons(layout);
        }
    }

    async fn prepare_video(&self, video: Arc<video::Video>) -> Result<VideoPainter> {
        let size = wgpu::Extent3d {
            width: video.width,
            height: video.height,
            depth_or_array_layers: 1,
        };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("MP4 frame"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let sampler = self.device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let layout = self
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("MP4 frame layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("MP4 frame bindings"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        let pipeline_layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("MP4 pipeline layout"),
                bind_group_layouts: &[&self.bind_layout, &layout],
                push_constant_ranges: &[],
            });
        let pipeline = compile(
            &self.device,
            &pipeline_layout,
            self.config.format,
            include_str!("video.wgsl"),
        )
        .await?;
        Ok(VideoPainter {
            video,
            texture,
            size,
            bind_group,
            pipeline,
            serial: 0,
        })
    }

    async fn prepare_shader(
        &self,
        source: &str,
        artwork: Option<&texture::Artwork>,
    ) -> Result<PreparedShader> {
        let textured = artwork
            .map(|artwork| texture::SceneTexture::new(&self.device, &self.queue, artwork))
            .transpose()?;
        let layout = textured.as_ref().map(|(layout, _)| {
            self.device
                .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("Textured wallpaper pipeline"),
                    bind_group_layouts: &[&self.bind_layout, layout],
                    push_constant_ranges: &[],
                })
        });
        let pipeline = compile(
            &self.device,
            layout.as_ref().unwrap_or(&self.layout),
            self.config.format,
            source,
        )
        .await?;
        Ok(PreparedShader {
            pipeline,
            texture: textured.map(|(_, texture)| texture),
        })
    }

    pub fn render(&mut self, width: u32, height: u32, time: f32) -> Result<bool> {
        self.render_with_inputs(width, height, time, &FrameInputs::default())
    }

    pub fn render_with_inputs(
        &mut self,
        width: u32,
        height: u32,
        time: f32,
        inputs: &FrameInputs,
    ) -> Result<bool> {
        if width == 0 || height == 0 {
            return Ok(false);
        }
        if (width, height) != (self.config.width, self.config.height) {
            self.config.width = width;
            self.config.height = height;
            self.surface.configure(&self.device, &self.config);
        }
        let frame = match self.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Outdated | wgpu::SurfaceError::Lost) => {
                self.surface.configure(&self.device, &self.config);
                return Ok(false);
            }
            Err(wgpu::SurfaceError::Timeout) => return Ok(false),
            Err(error) => return Err(error.into()),
        };

        // Keep the original 16-byte prefix so existing shaders still load.
        let mut values = [0.0; UNIFORM_BYTES as usize / 4];
        values[..3].copy_from_slice(&[width as f32, height as f32, time]);
        if let Some(cursor) = inputs.cursor {
            values[4..6].copy_from_slice(&cursor);
            values[6] = 1.0;
        }
        values[8..].copy_from_slice(&inputs.audio_bands);
        let uniforms = values.map(f32::to_le_bytes);
        self.queue
            .write_buffer(&self.globals, 0, uniforms.as_flattened());
        if let Some(video) = &mut self.video {
            video.upload(&self.queue);
        } else if let Some(texture) = &mut self.shader.texture {
            texture.upload(&self.queue);
        }
        let view = frame.texture.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            if let Some(video) = &self.video {
                pass.set_pipeline(&video.pipeline);
                pass.set_bind_group(1, &video.bind_group, &[]);
            } else {
                pass.set_pipeline(&self.shader.pipeline);
                if let Some(texture) = &self.shader.texture {
                    pass.set_bind_group(1, &texture.bind_group, &[]);
                }
            }
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit([encoder.finish()]);
        frame.present();
        Ok(true)
    }
}

struct VideoPainter {
    video: Arc<video::Video>,
    texture: wgpu::Texture,
    size: wgpu::Extent3d,
    bind_group: wgpu::BindGroup,
    pipeline: wgpu::RenderPipeline,
    serial: u64,
}

impl VideoPainter {
    fn upload(&mut self, queue: &wgpu::Queue) {
        if let Some(frame) = self.video.frame()
            && frame.serial != self.serial
        {
            queue.write_texture(
                self.texture.as_image_copy(),
                &frame.rgba,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.size.width * 4),
                    rows_per_image: Some(self.size.height),
                },
                self.size,
            );
            self.serial = frame.serial;
        }
    }
}

async fn compile(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    format: wgpu::TextureFormat,
    source: &str,
) -> Result<wgpu::RenderPipeline> {
    device.push_error_scope(wgpu::ErrorFilter::Validation);
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Wallpaper shader"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Wallpaper pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(format.into())],
        }),
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview: None,
        cache: None,
    });
    if let Some(error) = device.pop_error_scope().await {
        return Err(error.into());
    }
    Ok(pipeline)
}
