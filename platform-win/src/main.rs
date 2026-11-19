#[cfg(windows)]
mod app;
#[cfg(windows)]
mod desktop;
#[cfg(windows)]
mod ipc;
#[cfg(windows)]
mod shader;

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    if let Err(error) = app::run() {
        eprintln!("wallp: {error}");
        return std::process::ExitCode::FAILURE;
    }
    std::process::ExitCode::SUCCESS
}

#[cfg(not(windows))]
fn main() -> std::process::ExitCode {
    eprintln!("wallp: this pass supports Windows");
    std::process::ExitCode::FAILURE
}
