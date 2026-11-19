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
    use std::{ffi::OsStr, path::PathBuf};

    const USAGE: &str = "Usage: wallp [shader.wgsl | pack-folder]";
    let mut args = std::env::args_os().skip(1);
    let shader = args.next();
    if matches!(
        shader.as_deref().and_then(OsStr::to_str),
        Some("--help" | "-h")
    ) {
        println!(
            "{USAGE}\n\nRun on one display. Shader edits reload while wallp is running. Press Ctrl+C to stop."
        );
        return Ok(());
    }
    if shader.as_deref() == Some(OsStr::new("set")) {
        return Err(format!("{USAGE}\nPass the path when starting wallp on this platform.").into());
    }
    if args.next().is_some() {
        return Err(USAGE.into());
    }
    run_wallpaper(shader.map(PathBuf::from))
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn run() -> Result<(), Box<dyn std::error::Error>> {
    Err("No wallpaper backend is available for this operating system".into())
}
