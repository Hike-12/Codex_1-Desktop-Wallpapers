use std::{
    fs, io,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use serde::Deserialize;

use crate::{Renderer, Result, texture::Artwork, video::Video};

#[derive(Deserialize)]
struct Manifest {
    name: String,
    shader: Option<PathBuf>,
    video: Option<PathBuf>,
    artwork: Option<PathBuf>,
    #[serde(default)]
    desktop_icons: bool,
}

struct Pack {
    name: String,
    asset: PathBuf,
    artwork: Option<PathBuf>,
    desktop_icons: bool,
}

fn read_pack(folder: &Path) -> Result<Pack> {
    let manifest: Manifest = toml::from_str(&fs::read_to_string(folder.join("manifest.toml"))?)?;
    let name = manifest.name.trim();
    if name.is_empty() || name.chars().any(char::is_control) {
        return Err("Expected a wallpaper name on one line".into());
    }

    let (path, extensions) = match (manifest.shader, manifest.video) {
        (Some(path), None) => (path, &["wgsl"][..]),
        (None, Some(path)) => (path, VIDEO_EXTENSIONS),
        _ => return Err("A wallpaper pack needs exactly one 'shader' or 'video' field".into()),
    };
    let root = folder.canonicalize()?;
    let asset = resolve_pack_asset(&root, &path, extensions)?;
    if (manifest.artwork.is_some() && is_video(&asset))
        || (manifest.desktop_icons && manifest.artwork.is_none())
    {
        return Err("Artwork requires a shader, and desktop icons require artwork".into());
    }
    let artwork = manifest
        .artwork
        .map(|path| resolve_pack_asset(&root, &path, &["jpg", "jpeg", "png", "webp"]))
        .transpose()?;
    Ok(Pack {
        name: name.to_owned(),
        asset,
        artwork,
        desktop_icons: manifest.desktop_icons,
    })
}

const VIDEO_EXTENSIONS: &[&str] = &["mp4", "webm", "mkv"];

pub(crate) fn is_video(path: &Path) -> bool {
    VIDEO_EXTENSIONS
        .iter()
        .any(|extension| has_extension(path, extension))
}

pub(crate) fn pack_artwork(path: &Path) -> Result<PathBuf> {
    read_pack(path)?
        .artwork
        .ok_or_else(|| "No artwork preview".into())
}

fn resolve_pack_asset(root: &Path, path: &Path, extensions: &[&str]) -> Result<PathBuf> {
    if !extensions
        .iter()
        .any(|ext| validate_pack_path(path, ext).is_ok())
    {
        return Err(format!(
            "Expected a relative {} file inside the pack",
            extensions.join("/")
        )
        .into());
    }
    let asset = root.join(path).canonicalize()?;
    if !asset.starts_with(root) || !asset.is_file() {
        return Err("The asset must be a file inside its wallpaper pack".into());
    }
    Ok(asset)
}

fn has_extension(path: &Path, extension: &str) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case(extension))
}

fn validate_pack_path(path: &Path, extension: &str) -> Result<()> {
    if has_extension(path, extension)
        && path
            .components()
            .all(|part| matches!(part, Component::CurDir | Component::Normal(_)))
    {
        return Ok(());
    }
    Err(format!("Expected a relative .{extension} path inside the pack").into())
}

pub fn installed_packs(root: &Path) -> io::Result<Vec<(String, PathBuf)>> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut packs = Vec::new();
    for entry in entries {
        let folder = entry?.path();
        if folder.is_dir() {
            match read_pack(&folder) {
                Ok(pack) => packs.push((pack.name, folder)),
                Err(error) => eprintln!("wallp: {}: {error}", folder.display()),
            }
        }
    }
    packs.sort();
    Ok(packs)
}

pub struct SceneFile {
    path: PathBuf,
    source: SceneSource,
}

enum SceneSource {
    Shader(ShaderFile),
    Video {
        playback: Option<Arc<Video>>,
        attempted: bool,
    },
}

struct ShaderFile {
    path: PathBuf,
    artwork: Option<PathBuf>,
    desktop_icons: bool,
    last_read: Option<std::result::Result<Arc<str>, String>>,
    last_good: Option<(Arc<str>, Option<Artwork>)>,
}

impl SceneFile {
    pub fn resolve(path: PathBuf) -> Result<Self> {
        let (path, artwork, desktop_icons) = if path.is_dir() {
            let pack = read_pack(&path).map_err(|error| format!("{}: {error}", path.display()))?;
            (pack.asset, pack.artwork, pack.desktop_icons)
        } else {
            (path, None, false)
        };
        let source = if has_extension(&path, "wgsl") {
            SceneSource::Shader(ShaderFile {
                path: path.clone(),
                artwork,
                desktop_icons,
                last_read: None,
                last_good: None,
            })
        } else if is_video(&path) {
            SceneSource::Video {
                playback: None,
                attempted: false,
            }
        } else {
            return Err(format!(
                "{}: expected a WGSL file, a supported video, or a pack folder",
                path.display()
            )
            .into());
        };
        Ok(Self { path, source })
    }

    pub fn select_all(mut self, renderers: &mut [&mut Renderer<'_>]) -> Result<Self> {
        match &mut self.source {
            SceneSource::Shader(shader) => shader.select_all(renderers)?,
            SceneSource::Video {
                playback,
                attempted,
            } => {
                *playback = Some(load_video(&self.path, renderers)?);
                *attempted = true;
            }
        }
        Ok(self)
    }

    pub fn apply_to(&self, renderer: &mut Renderer<'_>) -> Result<()> {
        match &self.source {
            SceneSource::Shader(shader) => shader.apply_to(renderer),
            SceneSource::Video {
                playback: Some(video),
                ..
            } => {
                renderer.video = Some(pollster::block_on(renderer.prepare_video(video.clone()))?);
                Ok(())
            }
            SceneSource::Video { playback: None, .. } => Ok(()),
        }
    }

    pub fn reload_all(&mut self, renderers: &mut [&mut Renderer<'_>]) {
        if renderers.is_empty() {
            return;
        }
        match &mut self.source {
            SceneSource::Shader(shader) => shader.reload_all(renderers),
            SceneSource::Video {
                playback,
                attempted,
            } if !*attempted => {
                *attempted = true;
                match load_video(&self.path, renderers) {
                    Ok(video) => *playback = Some(video),
                    Err(error) => eprintln!("wallp: {}: {error}", self.path.display()),
                }
            }
            SceneSource::Video { .. } => {}
        }
    }

    pub fn set_active(&self, active: bool) {
        if let SceneSource::Video {
            playback: Some(video),
            ..
        } = &self.source
        {
            video.set_active(active);
        }
    }
    pub fn has_video(&self) -> bool {
        matches!(
            &self.source,
            SceneSource::Video {
                playback: Some(_),
                ..
            }
        )
    }

    pub fn uses_desktop_icons(&self) -> bool {
        matches!(&self.source, SceneSource::Shader(shader) if shader.desktop_icons && shader.last_good.is_some())
    }
}

impl ShaderFile {
    fn install(&mut self, source: Arc<str>, renderers: &mut [&mut Renderer<'_>]) -> Result<()> {
        let artwork = self
            .artwork
            .as_deref()
            .map(|path| {
                Artwork::load(path, self.desktop_icons)
                    .map_err(|error| format!("{}: {error}", path.display()))
            })
            .transpose()?;
        set_shader_with_artwork(&source, artwork.as_ref(), renderers)?;
        self.last_good = Some((source, artwork));
        Ok(())
    }

    fn select_all(&mut self, renderers: &mut [&mut Renderer<'_>]) -> Result<()> {
        let source: Arc<str> = fs::read_to_string(&self.path)
            .map(Arc::from)
            .map_err(|error| format!("{}: {error}", self.path.display()))?;
        self.install(source.clone(), renderers)
            .map_err(|error| format!("{}: {error}", self.path.display()))?;
        self.last_read = Some(Ok(source));
        eprintln!("wallp: loaded {}", self.path.display());
        Ok(())
    }

    fn apply_to(&self, renderer: &mut Renderer<'_>) -> Result<()> {
        if let Some((source, artwork)) = &self.last_good {
            renderer.shader =
                pollster::block_on(renderer.prepare_shader(source, artwork.as_ref()))?;
            renderer.video = None;
        }
        Ok(())
    }

    fn reload_all(&mut self, renderers: &mut [&mut Renderer<'_>]) {
        let read = fs::read_to_string(&self.path)
            .map(Arc::from)
            .map_err(|error| error.to_string());
        if self.last_read.as_ref() == Some(&read) {
            return;
        }
        match &read {
            Ok(source) => match self.install(source.clone(), renderers) {
                Ok(()) => {
                    eprintln!("wallp: loaded {}", self.path.display());
                }
                Err(error) => eprintln!("wallp: {}: {error}", self.path.display()),
            },
            Err(error) => eprintln!("wallp: {}: {error}", self.path.display()),
        }
        self.last_read = Some(read);
    }
}

fn load_video(path: &Path, renderers: &mut [&mut Renderer<'_>]) -> Result<Arc<Video>> {
    if renderers.is_empty() {
        return Err("No wallpaper displays are available".into());
    }
    let video = Video::open(path)?;
    let painters = pollster::block_on(async {
        let mut painters = Vec::with_capacity(renderers.len());
        for renderer in renderers.iter() {
            painters.push(renderer.prepare_video(video.clone()).await?);
        }
        Ok::<_, Box<dyn std::error::Error>>(painters)
    })?;
    for (renderer, painter) in renderers.iter_mut().zip(painters) {
        renderer.shader.texture = None;
        renderer.video = Some(painter);
    }
    eprintln!("wallp: loaded {}", path.display());
    Ok(video)
}

/// Compile each display's pipeline before changing any of them.
pub fn set_shader_all(source: &str, renderers: &mut [&mut Renderer<'_>]) -> Result<()> {
    set_shader_with_artwork(source, None, renderers)
}

fn set_shader_with_artwork(
    source: &str,
    artwork: Option<&Artwork>,
    renderers: &mut [&mut Renderer<'_>],
) -> Result<()> {
    if renderers.is_empty() {
        return Err("No wallpaper displays are available".into());
    }
    let pipelines = pollster::block_on(async {
        let mut pipelines = Vec::with_capacity(renderers.len());
        for renderer in renderers.iter() {
            pipelines.push(renderer.prepare_shader(source, artwork).await?);
        }
        Ok::<_, Box<dyn std::error::Error>>(pipelines)
    })?;
    for (renderer, pipeline) in renderers.iter_mut().zip(pipelines) {
        renderer.shader = pipeline;
        renderer.video = None;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_shader_must_stay_inside_the_pack() {
        assert!(validate_pack_path(Path::new("./effects/sky.WGSL"), "wgsl").is_ok());
        assert!(validate_pack_path(Path::new("./clips/sky.MP4"), "mp4").is_ok());
        assert!(validate_pack_path(Path::new("../other.wgsl"), "wgsl").is_err());
        assert!(validate_pack_path(Path::new("/other.wgsl"), "wgsl").is_err());
        assert!(validate_pack_path(Path::new("clip.mp4"), "wgsl").is_err());
    }
}
