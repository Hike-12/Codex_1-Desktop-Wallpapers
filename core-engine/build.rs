use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=../wallpapers");
    println!("cargo:rerun-if-env-changed=WALLP_FFMPEG_DIR");
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let mut files = String::from("&[\n");
    let mut packs = fs::read_dir("../wallpapers")
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.join("manifest.toml").is_file())
        .collect::<Vec<_>>();
    packs.sort();
    assert!(!packs.is_empty(), "missing bundled wallpaper packs");
    for pack in packs {
        let mut entries = fs::read_dir(&pack)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.is_file())
            .collect::<Vec<_>>();
        entries.sort();
        for path in entries {
            let relative = path.strip_prefix("../wallpapers").unwrap();
            files.push_str(&format!(
                "({:?}, include_bytes!({:?})),\n",
                relative.to_str().unwrap(),
                path.canonicalize().unwrap().to_str().unwrap()
            ));
        }
    }
    files.push_str("]\n");
    fs::write(output.join("wallpapers.rs"), files).unwrap();

    let mut tools = String::from("&[\n");
    if let Some(directory) = env::var_os("WALLP_FFMPEG_DIR") {
        for name in ["ffmpeg", "ffprobe"] {
            let filename = if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
                format!("{name}.exe")
            } else {
                name.to_owned()
            };
            let path = PathBuf::from(&directory)
                .join(&filename)
                .canonicalize()
                .unwrap();
            println!("cargo:rerun-if-changed={}", path.display());
            tools.push_str(&format!(
                "({name:?}, {filename:?}, include_bytes!({:?})),\n",
                path.to_str().unwrap()
            ));
        }
    }
    tools.push_str("]\n");
    fs::write(output.join("video_tools.rs"), tools).unwrap();
}
