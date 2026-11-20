use std::{
    path::Path,
    time::{Duration, Instant},
};

use crate::{
    Result,
    icons::{FIELD_BYTES, FIELD_HEIGHT, FIELD_WIDTH, IconLayout},
};

const TRANSITION: Duration = Duration::from_millis(650);
const MAX_ARTWORK_EDGE: u32 = 4096;

pub(crate) struct Artwork {
    pixels: image::RgbaImage,
    desktop_icons: bool,
}

impl Artwork {
    pub(crate) fn load(path: &Path, desktop_icons: bool) -> Result<Self> {
        Ok(Self {
            pixels: Self::read(path)?.into_rgba8(),
            desktop_icons,
        })
    }

    pub(crate) fn read(path: &Path) -> Result<image::DynamicImage> {
        let mut reader = image::ImageReader::open(path)?.with_guessed_format()?;
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(MAX_ARTWORK_EDGE);
        limits.max_image_height = Some(MAX_ARTWORK_EDGE);
        limits.max_alloc = Some(128 * 1024 * 1024);
        reader.limits(limits);
        Ok(reader.decode()?)
    }
}

pub(crate) struct SceneTexture {
    pub(crate) bind_group: wgpu::BindGroup,
    fields: [wgpu::Texture; 2],
    icons: Option<IconLayout>,
    pixels: [Vec<u8>; 2],
    transition: Option<Transition>,
}

struct Transition {
    started: Instant,
    from: [Vec<u8>; 2],
    to: [Vec<u8>; 2],
}

impl SceneTexture {
    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        artwork: &Artwork,
    ) -> Result<(wgpu::BindGroupLayout, Self)> {
        let (width, height) = artwork.pixels.dimensions();
        let max = device.limits().max_texture_dimension_2d;
        if width > max || height > max {
            return Err(format!("Artwork exceeds this GPU's {max}-pixel texture limit").into());
        }
        let image = make_texture(
            device,
            "Wallpaper artwork",
            width,
            height,
            wgpu::TextureFormat::Rgba8UnormSrgb,
        );
        upload(queue, &image, artwork.pixels.as_raw());
        let fields = ["Desktop icon field", "Desktop link field"].map(|label| {
            make_texture(
                device,
                label,
                FIELD_WIDTH,
                FIELD_HEIGHT,
                wgpu::TextureFormat::Rgba8Unorm,
            )
        });
        let icons = artwork.desktop_icons.then(|| {
            if cfg!(windows) {
                IconLayout {
                    radius: [0.05, 0.05],
                    icons: Vec::new(),
                }
            } else {
                IconLayout::demo()
            }
        });
        let pixels = icons.as_ref().map_or_else(
            || [vec![0; FIELD_BYTES], vec![0; FIELD_BYTES]],
            |icons| [icons.rasterize(), icons.rasterize_links()],
        );
        for (field, pixels) in fields.iter().zip(&pixels) {
            upload(queue, field, pixels);
        }
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let sampled = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Wallpaper artwork layout"),
            entries: &[
                sampled(0),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                sampled(2),
                sampled(3),
            ],
        });
        let image_view = image.create_view(&Default::default());
        let [icon_view, link_view] = fields
            .each_ref()
            .map(|field| field.create_view(&Default::default()));
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Wallpaper artwork bindings"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&image_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&icon_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&link_view),
                },
            ],
        });
        Ok((
            layout,
            Self {
                bind_group,
                fields,
                icons,
                pixels,
                transition: None,
            },
        ))
    }

    pub(crate) fn update_icons(&mut self, layout: &IconLayout) {
        if self.icons.as_ref().is_none_or(|icons| icons == layout) {
            return;
        }
        self.icons = Some(layout.clone());
        self.transition = Some(Transition {
            started: Instant::now(),
            from: self.pixels.clone(),
            to: [layout.rasterize(), layout.rasterize_links()],
        });
    }

    pub(crate) fn upload(&mut self, queue: &wgpu::Queue) {
        let Some(transition) = &self.transition else {
            return;
        };
        let t = (transition.started.elapsed().as_secs_f32() / TRANSITION.as_secs_f32()).min(1.0);
        let blend = t * t * (3.0 - 2.0 * t);
        for (index, field) in self.fields.iter().enumerate() {
            let pixels = &mut self.pixels[index];
            blend_field(
                pixels,
                &transition.from[index],
                &transition.to[index],
                blend,
            );
            upload(queue, field, pixels);
        }
        if t >= 1.0 {
            self.transition = None;
        }
    }
}

fn blend_field(pixels: &mut [u8], from: &[u8], to: &[u8], blend: f32) {
    for ((output, from), to) in pixels
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(from.as_chunks::<4>().0)
        .zip(to.as_chunks::<4>().0)
    {
        for channel in [0, 2, 3] {
            let from = f32::from(from[channel]);
            let to = f32::from(to[channel]);
            output[channel] = (from + (to - from) * blend).round() as u8;
        }
        output[1] = if f32::from(to[2]) * blend >= f32::from(from[2]) * (1.0 - blend) {
            to[1]
        } else {
            from[1]
        };
    }
}

fn make_texture(
    device: &wgpu::Device,
    label: &str,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

fn upload(queue: &wgpu::Queue, texture: &wgpu::Texture, rgba: &[u8]) {
    queue.write_texture(
        texture.as_image_copy(),
        rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(texture.width() * 4),
            rows_per_image: Some(texture.height()),
        },
        texture.size(),
    );
}
