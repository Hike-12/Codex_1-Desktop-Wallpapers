#![cfg_attr(windows, windows_subsystem = "windows")]

use std::process::ExitCode;

fn main() -> ExitCode {
    #[cfg(windows)]
    if std::env::args_os().len() > 1 {
        use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
        unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
    }
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--licenses")) {
        println!("{}", include_str!("../../scripts/ffmpeg-notice.txt"));
        println!("{}", include_str!("../../scripts/COPYING.LGPLv2.1"));
        return ExitCode::SUCCESS;
    }
    if let Err(error) = run() {
        eprintln!("wallp: {error}");
        #[cfg(windows)]
        if std::env::args_os()
            .nth(1)
            .is_none_or(|arg| arg == "--gallery" || arg == "--background")
        {
            platform_win::show_error(&error.to_string());
        }
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
                    "{USAGE}\n\n  --audio          React to system playback audio\n  --battery-saver  Pause rendering on battery\n\nShader edits reload while wallp is running.\nPress Ctrl+C to stop."
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
    let root = core_engine::library::folder()?;
    core_engine::library::install_built_ins(&root)?;
    let maple = root.join("maple-drift");
    run_wallpaper(
        shader.or_else(|| maple.is_dir().then_some(maple)),
        audio,
        battery_saver,
    )
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn run() -> Result<(), Box<dyn std::error::Error>> {
    Err("No wallpaper backend is available for this operating system".into())
}
