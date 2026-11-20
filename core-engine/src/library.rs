use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

use crate::{Result, shader, texture, video};

const BUILT_INS: &[(&str, &[u8])] = include!(concat!(env!("OUT_DIR"), "/wallpapers.rs"));
pub const MEDIA_EXTENSIONS: &[&str] = &["mp4", "webm", "mkv", "jpg", "png", "webp"];

pub fn folder() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("WALLP_DATA_DIR") {
        return Ok(std::path::absolute(path)?.join("wallpapers"));
    }
    Ok(dirs::data_dir()
        .ok_or("Could not find your wallpaper data directory")?
        .join("wallp")
        .join("wallpapers"))
}

/// Publish a complete library in one rename so an interrupted first run can retry.
pub fn install_built_ins(root: &Path) -> Result<bool> {
    match root.read_dir() {
        Ok(mut entries) => {
            if entries.next().transpose()?.is_some() {
                return Ok(false);
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let parent = root.parent().ok_or("Invalid wallpaper library directory")?;
    fs::create_dir_all(parent)?;
    let staged = tempfile::tempdir_in(parent)?;
    for (name, bytes) in BUILT_INS {
        let path = staged.path().join(name);
        fs::create_dir_all(path.parent().unwrap())?;
        fs::write(path, bytes)?;
    }
    match fs::remove_dir(root) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) if error.kind() == io::ErrorKind::DirectoryNotEmpty => return Ok(false),
        Err(error) => return Err(error.into()),
    }
    match fs::rename(staged.path(), root) {
        Ok(()) => Ok(true),
        Err(_)
            if root
                .read_dir()
                .is_ok_and(|mut entries| entries.next().is_some()) =>
        {
            Ok(false)
        }
        Err(error) => Err(error.into()),
    }
}

pub fn thumbnail(pack: &Path) -> Result<image::DynamicImage> {
    let path = pack.join("preview.jpg");
    let pixels = texture::Artwork::read(&path)
        .or_else(|_| texture::Artwork::read(&shader::pack_artwork(pack)?))?;
    Ok(pixels.thumbnail(480, 270))
}

/// Remove a pack from the gallery while keeping its files recoverable.
pub fn trash_pack(root: &Path, pack: &Path) -> Result<()> {
    let root = root.canonicalize()?;
    let pack = pack.canonicalize()?;
    if pack.parent() != Some(root.as_path()) || !pack.join("manifest.toml").is_file() {
        return Err("Choose a wallpaper installed in the gallery".into());
    }
    let parent = root.parent().ok_or("Invalid wallpaper library directory")?;
    let trash = parent.join("deleted-wallpapers");
    fs::create_dir_all(&trash)?;
    let destination = tempfile::Builder::new().prefix("pack-").tempdir_in(trash)?;
    // Keep the library initialized even after the last pack is removed.
    fs::File::create(root.join(".initialized"))?;
    fs::rename(&pack, destination.path().join(pack.file_name().unwrap()))?;
    let _ = destination.keep();
    Ok(())
}

pub fn import(root: &Path, source: &Path) -> Result<PathBuf> {
    let extension = source
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !source.is_file() || !MEDIA_EXTENSIONS.contains(&extension.as_str()) {
        return Err("Choose an MP4, WebM, MKV, JPG, PNG or WebP file".into());
    }
    let name: String = source
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .chars()
        .filter(|c| !c.is_control())
        .take(100)
        .collect();
    let name = if name.trim().is_empty() {
        "Imported wallpaper"
    } else {
        name.trim()
    };
    let parent = root.parent().ok_or("Invalid wallpaper library directory")?;
    fs::create_dir_all(root)?;
    let staged = tempfile::Builder::new()
        .prefix("import-")
        .tempdir_in(parent)?;
    let filename = format!("media.{extension}");
    let asset = staged.path().join(&filename);
    fs::copy(source, &asset)?;
    let preview = staged.path().join("preview.jpg");
    let mut manifest = toml::Table::new();
    manifest.insert("name".into(), name.into());
    if shader::is_video(&asset) {
        video::thumbnail(&asset, &preview)?;
        manifest.insert("video".into(), filename.into());
    } else {
        texture::Artwork::read(&asset)?
            .thumbnail(480, 270)
            .into_rgb8()
            .save(&preview)?;
        fs::write(staged.path().join("image.wgsl"), include_str!("video.wgsl"))?;
        manifest.insert("shader".into(), "image.wgsl".into());
        manifest.insert("artwork".into(), filename.into());
    }
    fs::write(
        staged.path().join("manifest.toml"),
        toml::to_string(&manifest)?,
    )?;
    let destination = root.join(staged.path().file_name().unwrap());
    fs::rename(staged.path(), &destination)?;
    Ok(destination)
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path.parent().ok_or(io::ErrorKind::InvalidInput)?;
    fs::create_dir_all(parent)?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)?;
    staged.write_all(bytes)?;
    staged.persist(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deleting_the_last_pack_keeps_it_recoverable_and_does_not_reseed() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("wallpapers");
        let pack = root.join("example");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&pack)?;
        fs::create_dir(&outside)?;
        fs::write(pack.join("manifest.toml"), b"name = 'Example'")?;
        fs::write(outside.join("manifest.toml"), b"name = 'Outside'")?;
        assert!(trash_pack(&root, &outside).is_err());
        assert!(outside.is_dir());
        trash_pack(&root, &pack)?;
        assert!(!pack.exists());
        assert!(!install_built_ins(&root)?);
        let recovery = fs::read_dir(temp.path().join("deleted-wallpapers"))?
            .next()
            .expect("one deleted wallpaper")?
            .path();
        assert!(recovery.join("example/manifest.toml").is_file());
        Ok(())
    }

    #[test]
    fn first_run_and_image_import_publish_complete_packs() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("wallpapers");
        fs::create_dir(&root)?;
        assert!(install_built_ins(&root)?);
        assert_eq!(shader::installed_packs(&root)?.len(), 15);
        assert!(root.join("maple-drift/preview.jpg").is_file());
        let source = temp.path().join("Rain & Sky.png");
        image::RgbImage::new(12, 20).save(&source)?;
        let pack = import(&root, &source)?;
        assert_eq!(fs::read(pack.join("media.png"))?, fs::read(&source)?);
        assert!(pack.join("preview.jpg").is_file());
        assert!(!install_built_ins(&root)?);
        assert_eq!(shader::installed_packs(&root)?.len(), 16);
        let _ = shader::SceneFile::resolve(pack)?;
        assert!(import(&root, &temp.path().join("unknown.bin")).is_err());
        Ok(())
    }
}
