use std::{
    fs, io,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use serde::Deserialize;

use crate::{Renderer, Result, video::Video};

#[derive(Deserialize)]
struct Manifest {
    name: String,
    shader: Option<PathBuf>,
    video: Option<PathBuf>,
}

fn read_pack(folder: &Path) -> Result<(String, PathBuf)> {
    let manifest: Manifest = toml::from_str(&fs::read_to_string(folder.join("manifest.toml"))?)?;
    let name = manifest.name.trim();
    if name.is_empty() || name.chars().any(char::is_control) {
        return Err("Expected a wallpaper name on one line".into());
    }

    let (path, ext) = match (manifest.shader, manifest.video) {
        (Some(path), None) => (path, "wgsl"),
        (None, Some(path)) => (path, "mp4"),
        _ => return Err("A wallpaper pack needs exactly one 'shader' or 'video' field".into()),
    };
    validate_pack_path(&path, ext)?;
    let root = folder.canonicalize()?;
    let asset = root.join(path).canonicalize()?;
    if !asset.starts_with(&root) || !asset.is_file() {
        return Err("The asset must be a file inside its wallpaper pack".into());
    }
    Ok((name.to_owned(), asset))
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
                Ok((name, _)) => packs.push((name, folder)),
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
    last_read: Option<std::result::Result<Arc<str>, String>>,
    last_good: Option<Arc<str>>,
}

impl SceneFile {
    pub fn resolve(path: PathBuf) -> Result<Self> {
        let path = if path.is_dir() {
            read_pack(&path)
                .map_err(|error| format!("{}: {error}", path.display()))?
                .1
        } else {
            path
        };
        let source = if has_extension(&path, "wgsl") {
            SceneSource::Shader(ShaderFile {
                path: path.clone(),
                last_read: None,
                last_good: None,
            })
        } else if has_extension(&path, "mp4") {
            SceneSource::Video {
                playback: None,
                attempted: false,
            }
        } else {
            return Err(format!(
                "{}: expected a .wgsl file, .mp4 file, or pack folder",
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
}

impl ShaderFile {
    fn select_all(&mut self, renderers: &mut [&mut Renderer<'_>]) -> Result<()> {
        let source: Arc<str> = fs::read_to_string(&self.path)
            .map(Arc::from)
            .map_err(|error| format!("{}: {error}", self.path.display()))?;
        set_shader_all(&source, renderers)
            .map_err(|error| format!("{}: {error}", self.path.display()))?;
        self.last_good = Some(source.clone());
        self.last_read = Some(Ok(source));
        eprintln!("wallp: loaded {}", self.path.display());
        Ok(())
    }

    fn apply_to(&self, renderer: &mut Renderer<'_>) -> Result<()> {
        if let Some(source) = &self.last_good {
            pollster::block_on(renderer.set_shader(source))?;
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
            Ok(source) => match set_shader_all(source, renderers) {
                Ok(()) => {
                    self.last_good = Some(source.clone());
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
        renderer.video = Some(painter);
    }
    eprintln!("wallp: loaded {}", path.display());
    Ok(video)
}

/// Compile each display's pipeline before changing any of them.
pub fn set_shader_all(source: &str, renderers: &mut [&mut Renderer<'_>]) -> Result<()> {
    if renderers.is_empty() {
        return Err("No wallpaper displays are available".into());
    }
    let pipelines = pollster::block_on(async {
        let mut pipelines = Vec::with_capacity(renderers.len());
        for renderer in renderers.iter() {
            pipelines.push(renderer.prepare_shader(source).await?);
        }
        Ok::<_, Box<dyn std::error::Error>>(pipelines)
    })?;
    for (renderer, pipeline) in renderers.iter_mut().zip(pipelines) {
        renderer.pipeline = pipeline;
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
