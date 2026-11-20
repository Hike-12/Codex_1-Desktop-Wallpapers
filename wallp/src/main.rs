use std::process::ExitCode;

fn main() -> ExitCode {
    if let Err(error) = run() {
        eprintln!("wallp: {error}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

#[cfg(windows)]
use platform_win::run;

#[cfg(target_os = "linux")]
use platform_linux::run as run_wallpaper;

#[cfg(target_os = "macos")]
use platform_mac::run as run_wallpaper;

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn run() -> Result<(), Box<dyn std::error::Error>> {
    use std::path::PathBuf;

    const USAGE: &str =
        "Usage: wallp [--audio] [--battery-saver] [shader.wgsl | clip.mp4 | pack-folder]";
    let mut shader = None;
    let mut audio = false;
    let mut battery_saver = false;
    let mut positional = false;
    for arg in std::env::args_os().skip(1) {
        match arg.as_os_str().to_str().filter(|_| !positional) {
            Some("--") => positional = true,
            Some("--audio") => audio = true,
            Some("--battery-saver") => battery_saver = true,
            Some("--help" | "-h") => {
                println!(
                    "{USAGE}\n\n  --audio          React to system playback audio\n  --battery-saver  Pause rendering on battery\n\nShader edits reload while wallp is running. MP4 needs FFmpeg on PATH.\nPress Ctrl+C to stop."
                );
                return Ok(());
            }
            Some("set") if shader.is_none() => {
                return Err(format!(
                    "{USAGE}\nPass the path when starting wallp on this platform."
                )
                .into());
            }
            Some(option) if option.starts_with('-') => {
                return Err(format!("Unknown option '{option}'.\n{USAGE}").into());
            }
            _ if shader.is_none() => shader = Some(PathBuf::from(arg)),
            _ => return Err(USAGE.into()),
        }
    }
    run_wallpaper(shader, audio, battery_saver)
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn run() -> Result<(), Box<dyn std::error::Error>> {
    Err("No wallpaper backend is available for this operating system".into())
}
