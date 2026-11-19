# Desktop Wallpapers

- Keep changes focused on the agreed pass. Build and verify a working feature before extending it.
- Keep rendering and WGSL in `core-engine`. Native desktop placement, window lifetime, and fullscreen detection belong in the platform backend.
- Keep Win32 calls local to the Windows backend and document assumptions about Explorer's undocumented desktop hierarchy.
- Preserve the active shader when a reload fails. Stop requesting frames while a fullscreen window covers the wallpaper monitor.
- Run `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace` before closing a pass. Verify desktop behavior on the target OS.
- Commit working checkpoints with short messages that describe what changed. Commit the full pass and push it to GitHub when verified.
