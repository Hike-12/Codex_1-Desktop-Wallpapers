use std::{
    fs, io,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use serde::Deserialize;

use crate::{Renderer, Result};

#[derive(Deserialize)]
struct Manifest {
    name: String,
    shader: PathBuf,
}

fn read_pack(folder: &Path) -> Result<(String, PathBuf)> {
    let manifest: Manifest = toml::from_str(&fs::read_to_string(folder.join("manifest.toml"))?)?;
    let name = manifest.name.trim();
    if name.is_empty() || name.chars().any(char::is_control) {
        return Err("Expected a wallpaper name on one line".into());
    }

    validate_pack_shader(&manifest.shader)?;
    let root = folder.canonicalize()?;
    let shader = root.join(manifest.shader).canonicalize()?;
    if !shader.starts_with(&root) || !shader.is_file() {
        return Err("The WGSL shader must be a file inside its wallpaper pack".into());
    }
    Ok((name.to_owned(), shader))
}

fn is_wgsl(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("wgsl"))
}

fn validate_pack_shader(path: &Path) -> Result<()> {
    if is_wgsl(path)
        && path
            .components()
            .all(|part| matches!(part, Component::CurDir | Component::Normal(_)))
    {
        return Ok(());
    }
    Err("Pack 'shader' must be a relative .wgsl path inside the pack".into())
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

#[derive(Clone)]
pub struct ShaderFile {
    path: PathBuf,
    last_read: Option<std::result::Result<Arc<str>, String>>,
    last_good: Option<Arc<str>>,
}

impl ShaderFile {
    pub fn resolve(path: PathBuf) -> Result<Self> {
        let path = if path.is_dir() {
            read_pack(&path)
                .map_err(|error| format!("{}: {error}", path.display()))?
                .1
        } else if is_wgsl(&path) {
            path
        } else {
            return Err(
                format!("{}: expected a .wgsl file or a pack folder", path.display()).into(),
            );
        };
        Ok(Self {
            path,
            last_read: None,
            last_good: None,
        })
    }

    pub fn select(self, renderer: &mut Renderer<'_>) -> Result<Self> {
        self.select_all(&mut [renderer])
    }

    pub fn select_all(mut self, renderers: &mut [&mut Renderer<'_>]) -> Result<Self> {
        let source: Arc<str> = fs::read_to_string(&self.path)
            .map(Arc::from)
            .map_err(|error| format!("{}: {error}", self.path.display()))?;
        set_shader_all(&source, renderers)
            .map_err(|error| format!("{}: {error}", self.path.display()))?;
        self.last_good = Some(source.clone());
        self.last_read = Some(Ok(source));
        eprintln!("wallp: loaded {}", self.path.display());
        Ok(self)
    }

    pub fn apply_to(&self, renderer: &mut Renderer<'_>) -> Result<()> {
        if let Some(source) = &self.last_good {
            pollster::block_on(renderer.set_shader(source))?;
        }
        Ok(())
    }

    pub fn reload(&mut self, renderer: &mut Renderer<'_>) {
        self.reload_all(&mut [renderer]);
    }

    pub fn reload_all(&mut self, renderers: &mut [&mut Renderer<'_>]) {
        if renderers.is_empty() {
            return;
        }
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
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_shader_must_stay_inside_the_pack() {
        assert!(validate_pack_shader(Path::new("./effects/sky.WGSL")).is_ok());
        assert!(validate_pack_shader(Path::new("../other.wgsl")).is_err());
        assert!(validate_pack_shader(Path::new("/other.wgsl")).is_err());
        assert!(validate_pack_shader(Path::new("clip.mp4")).is_err());
    }
}
