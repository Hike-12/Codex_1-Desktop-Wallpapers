# Desktop Wallpapers

Desktop Wallpapers (`wallp`) is a Rust and wgpu live wallpaper engine for Windows, macOS, and Linux. It renders WGSL scenes and looping local MP4s on native desktop surfaces and pauses each display when the desktop backend detects fullscreen coverage.

## Get started

Download the archive for your OS and CPU from the [latest release](https://github.com/Hike-12/Codex_1-Desktop-Wallpapers/releases/latest). Extract it and run the executable from a terminal.

| Platform | Package | Desktop requirements |
| --- | --- | --- |
| Windows x86-64 | `wallp-windows-x86_64.zip` | Windows 11 with Explorer. Start with `.\wallp.exe`. |
| macOS Apple Silicon or Intel | `wallp-macos-arm64.tar.gz` or `wallp-macos-x86_64.tar.gz` | A logged-in macOS desktop. Start with `./wallp`. The CLI is distributed without Developer ID notarization. |
| Linux x86-64 | `wallp-linux-x86_64.tar.gz` | An EWMH X11 window manager or a Wayland compositor with `zwlr_layer_shell_v1`. The binary is built on Ubuntu 24.04 and uses system graphics libraries and libpulse. Start with `./wallp`. |

Running with no arguments uses the built-in WGSL scene. Pass a `.wgsl` file, an `.mp4` file, or a pack directory to start with that wallpaper. Press Ctrl+C to stop. Video needs both `ffmpeg` and `ffprobe` on PATH. [FFmpeg downloads](https://ffmpeg.org/download.html)

On Windows, the system tray lists installed packs and can switch to the built-in wallpaper, refresh the list, or quit. With the app running, use `.\wallp.exe set <file-or-pack>` from another terminal to switch live. On macOS and Linux, choose a file or pack at launch. WGSL edits reload during playback on all three platforms.

## Wallpaper packs

Put a `manifest.toml` and the referenced asset in one folder. The manifest accepts one asset field. Use paths relative to the pack directory.

**WGSL**

```toml
name = "Nebula"
shader = "nebula.wgsl"
```

Use `vs_main` and `fs_main` as entry points. The uniform buffer is at group 0, binding 0. Its first 16 bytes contain pixel resolution, animation time in seconds, and padding. For cursor and playback bands, copy the layout from the [built-in WGSL scene](core-engine/src/default.wgsl).

**MP4**

```toml
name = "Ocean"
video = "ocean.mp4"
```

Video loops silently and fills each display with a centered crop. The upload is capped at 1280×720 and 30 fps. FFmpeg is asked to select a hardware decoder where available, with software decoding available as a fallback. [FFmpeg hardware decoding options](https://ffmpeg.org/ffmpeg.html)

On Windows, place pack folders in `%APPDATA%\wallp\wallpapers` and select **Refresh wallpapers** in the tray after adding them. A failed live selection leaves the current scene active.

## Playback and power

- `--audio` enables opt-in system playback capture for reactive shaders. Linux uses a PulseAudio-compatible output monitor. macOS uses Core Audio taps and needs a supported macOS version and System Audio Recording permission. [Apple Core Audio tap documentation](https://developer.apple.com/documentation/coreaudio/capturing-system-audio-with-core-audio-taps)
- `--battery-saver` pauses the scene while the machine reports battery power. Video decoding also stops when every output pauses and resumes from the last playback position.
- Wayland supplies a neutral cursor value. Fullscreen detection uses compositor toplevel information where available, with compositor frame pacing otherwise.

## Build and verify

Use stable Rust with Cargo. On Ubuntu, install the desktop build dependencies with:

```sh
sudo apt-get install libpulse-dev libwayland-dev libxkbcommon-dev libx11-dev pkg-config
```

```sh
cargo build --release --locked --bin wallp
cargo run --locked --bin wallp -- path/to/wallpaper
cargo test --workspace --locked
```

The workspace separates the shared renderer from the Explorer, AppKit, X11, and Wayland backends. [CI](https://github.com/Hike-12/Codex_1-Desktop-Wallpapers/actions/workflows/ci.yml) checks Windows, macOS, and Linux.

Windows desktop tests used Explorer. Linux desktop tests used X11/Openbox and headless Sway. macOS build checks cover Apple Silicon and Intel. Native Finder placement, audio consent, and physical battery transitions still need live verification.

### Fullscreen pause measurement

One Windows 11 debug run used a single 1536×960 display on AC power with system audio enabled. The test used the built-in WGSL scene.

| Process metric | Rendering | Fullscreen paused |
| --- | ---: | ---: |
| CPU share | 1.17% | 0.014% |
| Busiest GPU engine, average | 11.88% | 0% sampled |
| Working set | 274.5 MiB | 274.5 MiB |
