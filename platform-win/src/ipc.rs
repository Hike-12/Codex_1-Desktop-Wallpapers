use std::{
    ffi::OsString,
    io,
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use core_engine::Result;
use interprocess::{
    local_socket::{
        GenericNamespaced, ListenerOptions, Name, ToNsName,
        tokio::{Listener, Stream, prelude::*},
    },
    os::windows::{local_socket::ListenerOptionsExt, security_descriptor::SecurityDescriptor},
};
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    runtime::{Builder, Runtime},
    sync::oneshot,
    time::timeout,
};
use widestring::u16cstr;
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use winit::event_loop::EventLoopProxy;

use crate::app::Event;

const MAX_MESSAGE: usize = 128 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(3);
const SWITCH_TIMEOUT: Duration = Duration::from_secs(15);

pub fn listen(proxy: EventLoopProxy<Event>) -> Result<()> {
    let runtime = runtime()?;
    // Allow connections from the pipe owner and deny network logons.
    let security = SecurityDescriptor::deserialize(u16cstr!("D:P(D;;GA;;;NU)(A;;GA;;;OW)"))?;
    let listener = {
        let _context = runtime.enter();
        ListenerOptions::new()
            .name(socket_name()?)
            .security_descriptor(security)
            .create_tokio()
            .map_err(|error| {
                format!("Could not open the wallpaper socket (is wallp already running?): {error}")
            })?
    };
    thread::Builder::new()
        .name("wallp-ipc".into())
        .spawn(move || runtime.block_on(serve(listener, proxy)))?;
    Ok(())
}

pub fn set(path: &Path) -> Result<()> {
    let path = std::path::absolute(path)?;
    let payload: Vec<u8> = path
        .as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect();
    let reply = request(&payload)?;
    if !reply.is_empty() {
        return Err(String::from_utf8(reply)?.into());
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
pub enum Control {
    Status,
    OpenGallery,
    Pause(bool),
    Audio(bool),
    Fps(u8),
    Startup(bool),
}

#[derive(Clone, Serialize, Deserialize)]
pub struct State {
    pub active: Option<PathBuf>,
    pub paused: bool,
    pub audio: bool,
    pub fps: u8,
    pub startup: bool,
}

pub fn control(control: Control) -> Result<State> {
    let mut payload = vec![0, 0];
    serde_json::to_writer(&mut payload, &control)?;
    let reply = request(&payload)?;
    serde_json::from_slice::<std::result::Result<State, String>>(&reply)
        .map_err(|_| String::from_utf8_lossy(&reply).into_owned())?
        .map_err(Into::into)
}

fn request(payload: &[u8]) -> Result<Vec<u8>> {
    runtime()?.block_on(async {
        let mut stream = timeout(IO_TIMEOUT, Stream::connect(socket_name()?))
            .await
            .map_err(|_| "Timed out connecting to wallp")?
            .map_err(|error| format!("Could not connect to wallp. Start wallp first. {error}"))?;
        timeout(IO_TIMEOUT, write_message(&mut stream, payload)).await??;
        let reply = timeout(SWITCH_TIMEOUT + IO_TIMEOUT, read_message(&mut stream))
            .await
            .map_err(|_| "Timed out waiting for wallp to switch wallpapers")??;
        Ok(reply)
    })
}

async fn serve(listener: Listener, proxy: EventLoopProxy<Event>) {
    loop {
        let mut stream = match listener.accept().await {
            Ok(stream) => stream,
            Err(error) => {
                eprintln!("wallp: wallpaper socket: {error}");
                return;
            }
        };
        let reply = dispatch(&mut stream, &proxy)
            .await
            .unwrap_or_else(String::into_bytes);
        let _ = timeout(IO_TIMEOUT, write_message(&mut stream, &reply)).await;
    }
}

async fn dispatch(
    stream: &mut Stream,
    proxy: &EventLoopProxy<Event>,
) -> std::result::Result<Vec<u8>, String> {
    let payload = timeout(IO_TIMEOUT, read_message(stream))
        .await
        .map_err(|_| "Timed out reading the wallpaper path")?
        .map_err(|error| error.to_string())?;
    if let Some(payload) = payload.strip_prefix(&[0, 0]) {
        let control = serde_json::from_slice(payload).map_err(|e| e.to_string())?;
        let (reply, response) = oneshot::channel();
        proxy
            .send_event(Event::Control { control, reply })
            .map_err(|_| "wallp is closing")?;
        let result = timeout(SWITCH_TIMEOUT, response)
            .await
            .map_err(|_| "Timed out waiting for wallp")?
            .map_err(|_| "wallp is closing")?;
        return serde_json::to_vec(&result).map_err(|e| e.to_string());
    }
    let (pairs, remainder) = payload.as_chunks::<2>();
    let units: Vec<u16> = pairs.iter().copied().map(u16::from_le_bytes).collect();
    let path = PathBuf::from(OsString::from_wide(&units));
    if !remainder.is_empty() || units.contains(&0) || !path.is_absolute() {
        return Err("Expected an absolute Windows wallpaper path".into());
    }
    let (reply, response) = oneshot::channel();
    proxy
        .send_event(Event::Set { path, reply })
        .map_err(|_| "wallp is closing")?;
    timeout(SWITCH_TIMEOUT, response)
        .await
        .map_err(|_| "Timed out switching wallpapers")?
        .map_err(|_| "wallp is closing")??;
    Ok(Vec::new())
}

async fn read_message(stream: &mut Stream) -> io::Result<Vec<u8>> {
    let len = stream.read_u32().await? as usize;
    if len > MAX_MESSAGE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Wallpaper message is too large",
        ));
    }
    let mut bytes = vec![0; len];
    stream.read_exact(&mut bytes).await?;
    Ok(bytes)
}

async fn write_message(stream: &mut Stream, bytes: &[u8]) -> io::Result<()> {
    if bytes.len() > MAX_MESSAGE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Wallpaper message is too large",
        ));
    }
    stream.write_u32(bytes.len() as u32).await?;
    stream.write_all(bytes).await
}

fn socket_name() -> io::Result<Name<'static>> {
    let mut session = 0;
    if unsafe { ProcessIdToSessionId(std::process::id(), &mut session) } == 0 {
        return Err(io::Error::last_os_error());
    }
    format!("wallp-v1-{session}").to_ns_name::<GenericNamespaced>()
}

fn runtime() -> io::Result<Runtime> {
    Builder::new_current_thread().enable_all().build()
}
