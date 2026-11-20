use std::{
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex, mpsc},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crate::Result;

const FPS: u32 = 30;
const START_TIMEOUT: Duration = Duration::from_secs(3);
const RETRY_INTERVAL: Duration = Duration::from_secs(2);

pub(crate) struct Video {
    path: PathBuf,
    pub(crate) width: u32,
    pub(crate) height: u32,
    duration: f64,
    latest: SharedFrame,
    state: Mutex<DecoderState>,
}

type SharedFrame = Arc<Mutex<Option<Arc<Frame>>>>;

pub(crate) struct Frame {
    pub(crate) serial: u64,
    pub(crate) rgba: Vec<u8>,
    position: f64,
}

struct DecoderState {
    decoder: Option<Decoder>,
    hardware: bool,
    restart_at_beginning: bool,
    retry_at: Instant,
}

struct Decoder {
    child: Child,
    reader: Option<JoinHandle<()>>,
    seek_segment: bool,
    first_serial: u64,
}

impl Drop for Decoder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

impl Video {
    pub(crate) fn open(path: &Path) -> Result<Arc<Self>> {
        let path = path.canonicalize()?;
        if !path.is_file() {
            return Err("Expected an MP4 file".into());
        }
        let (width, height, duration) = probe(&path)?;
        let video = Self {
            path,
            width,
            height,
            duration,
            latest: Arc::default(),
            state: Mutex::new(DecoderState {
                decoder: None,
                hardware: true,
                restart_at_beginning: false,
                retry_at: Instant::now(),
            }),
        };
        let (decoder, hardware) = match video.preroll(true) {
            Ok(decoder) => (decoder, true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Err(error.into()),
            Err(_) => (video.preroll(false)?, false),
        };
        {
            let mut state = video.state.lock().expect("video state is available");
            state.hardware = hardware;
            state.decoder = Some(decoder);
        }
        Ok(Arc::new(video))
    }

    fn preroll(&self, hardware: bool) -> io::Result<Decoder> {
        let (decoder, first) = self.start(hardware, true)?;
        first.recv_timeout(START_TIMEOUT).map_err(|_| {
            io::Error::other(
                "FFmpeg did not decode an MP4 frame. Check the file and installed codecs.",
            )
        })?;
        Ok(decoder)
    }

    pub(crate) fn frame(&self) -> Option<Arc<Frame>> {
        self.latest
            .lock()
            .expect("video frames are available")
            .clone()
    }

    pub(crate) fn set_active(&self, active: bool) {
        let mut state = self.state.lock().expect("video state is available");
        if !active {
            state.decoder = None;
            return;
        }
        if let Some(decoder) = &state.decoder {
            if decoder
                .reader
                .as_ref()
                .is_some_and(|reader| !reader.is_finished())
            {
                return;
            }
            let seek_segment = decoder.seek_segment;
            let decoded = self
                .frame()
                .is_some_and(|frame| frame.serial > decoder.first_serial);
            if seek_segment {
                state.restart_at_beginning = true;
                state.retry_at = Instant::now();
            }
            if !seek_segment || !decoded {
                state.hardware = false;
            }
            state.decoder = None;
        }
        if Instant::now() < state.retry_at {
            return;
        }
        state.retry_at = Instant::now() + RETRY_INTERVAL;
        match self.start(state.hardware, state.restart_at_beginning) {
            Ok((decoder, _)) => {
                state.decoder = Some(decoder);
                state.restart_at_beginning = false;
            }
            Err(error) => {
                state.hardware = false;
                eprintln!("wallp: could not restart MP4 playback: {error}");
            }
        }
    }

    fn start(
        &self,
        hardware: bool,
        from_beginning: bool,
    ) -> io::Result<(Decoder, mpsc::Receiver<()>)> {
        let previous = self.frame();
        let (mut serial, mut position) = previous
            .as_ref()
            .map_or((0, 0.0), |frame| (frame.serial, frame.position));
        let first_serial = serial;
        if from_beginning {
            position = 0.0;
        }
        // A seeked decoder plays only the remainder. Some FFmpeg builds would
        // repeat that seek on every loop if looping were enabled here.
        let seek_segment = position > 0.0;
        let mut command = command("ffmpeg");
        command.args([
            "-v",
            "error",
            "-nostdin",
            "-protocol_whitelist",
            "file,pipe",
        ]);
        if hardware {
            command.args(["-hwaccel", "auto"]);
        }
        let filter = format!(
            "fps={FPS},scale={}:{}:flags=fast_bilinear,setsar=1",
            self.width, self.height,
        );
        let mut child = command
            .args([
                "-stream_loop",
                if seek_segment { "0" } else { "-1" },
                "-re",
                "-f",
                "mov",
                "-ss",
            ])
            .arg(format!("{position:.6}"))
            .arg("-i")
            .arg(&self.path)
            .args([
                "-map",
                "0:V:0",
                "-an",
                "-sn",
                "-dn",
                "-filter_threads",
                "1",
                "-vf",
            ])
            .arg(filter)
            .args([
                "-pix_fmt",
                "rgba",
                "-c:v",
                "rawvideo",
                "-threads:v",
                "1",
                "-f",
                "rawvideo",
                "pipe:1",
            ])
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!("Could not launch ffmpeg. Install FFmpeg and add it to PATH. {error}"),
                )
            })?;
        let mut stdout = child.stdout.take().expect("FFmpeg uses a stdout pipe");
        let latest = self.latest.clone();
        let frame_bytes = (self.width * self.height * 4) as usize;
        let duration = self.duration;
        let (started, first) = mpsc::channel();
        let reader = thread::Builder::new()
            .name("wallp-video".into())
            .spawn(move || {
                let mut bytes = vec![0; frame_bytes];
                let mut next_position = position;
                let mut started = Some(started);
                while stdout.read_exact(&mut bytes).is_ok() {
                    serial += 1;
                    let frame = Arc::new(Frame {
                        serial,
                        rgba: bytes,
                        position: next_position,
                    });
                    let old = latest
                        .lock()
                        .expect("video frames are available")
                        .replace(frame);
                    if let Some(started) = started.take() {
                        let _ = started.send(());
                    }
                    bytes = old
                        .and_then(|frame| Arc::try_unwrap(frame).ok())
                        .map_or_else(|| vec![0; frame_bytes], |frame| frame.rgba);
                    next_position = (next_position + 1.0 / f64::from(FPS)).rem_euclid(duration);
                }
            });
        match reader {
            Ok(reader) => Ok((
                Decoder {
                    child,
                    reader: Some(reader),
                    seek_segment,
                    first_serial,
                },
                first,
            )),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                Err(error)
            }
        }
    }
}

fn command(name: &str) -> Command {
    let mut command = Command::new(name);
    command.stdin(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

fn probe(path: &Path) -> Result<(u32, u32, f64)> {
    let mut child = command("ffprobe")
        .args([
            "-v",
            "error",
            "-protocol_whitelist",
            "file,pipe",
            "-f",
            "mov",
            "-select_streams",
            "V:0",
            "-show_entries",
            "stream=width,height,sample_aspect_ratio,duration:stream_side_data=rotation:format=duration",
            "-of",
            "default=noprint_wrappers=1",
        ])
        .arg(path)
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|error| {
            format!("Could not run ffprobe. Install FFmpeg and add it to PATH. {error}")
        })?;
    let deadline = Instant::now() + START_TIMEOUT;
    while child.try_wait()?.is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Timed out inspecting the MP4 file".into());
        }
        thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err("FFprobe could not read the MP4 file".into());
    }
    parse_probe(&String::from_utf8(output.stdout)?)
}

fn parse_probe(text: &str) -> Result<(u32, u32, f64)> {
    let key = |name: &str| {
        text.lines()
            .filter_map(|line| line.split_once('='))
            .find_map(|(field, value)| (field == name).then_some(value))
    };
    let size = |name| -> Result<f64> {
        let value: u32 = key(name)
            .ok_or("The MP4 contains no playable video stream")?
            .parse()?;
        if value == 0 {
            return Err("The MP4 has empty video dimensions".into());
        }
        Ok(f64::from(value))
    };
    let ratio = key("sample_aspect_ratio")
        .and_then(|ratio| ratio.split_once(':'))
        .and_then(|(a, b)| Some(a.parse::<f64>().ok()? / b.parse::<f64>().ok()?))
        .filter(|ratio| ratio.is_finite() && *ratio > 0.0)
        .unwrap_or(1.0);
    let (mut width, mut height) = (size("width")? * ratio, size("height")?);
    let rotation = key("rotation")
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or(0.0);
    if (rotation / 90.0).round().rem_euclid(2.0) == 1.0 {
        std::mem::swap(&mut width, &mut height);
    }
    let factor = (1280.0 / width).min(720.0 / height).min(1.0);
    let duration = text
        .lines()
        .filter_map(|line| line.strip_prefix("duration=")?.parse::<f64>().ok())
        .find(|duration| duration.is_finite() && *duration > 0.0)
        .ok_or("The MP4 has no valid duration")?;
    Ok((
        (width * factor).round().max(1.0) as u32,
        (height * factor).round().max(1.0) as u32,
        duration,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoder_dimensions_fit_the_frame_budget_and_honor_rotation() {
        assert_eq!(
            parse_probe("width=3840\nheight=2160\nsample_aspect_ratio=1:1\nduration=2.5").unwrap(),
            (1280, 720, 2.5)
        );
        assert_eq!(
            parse_probe("width=1920\nheight=1080\nrotation=-90\nduration=1").unwrap(),
            (405, 720, 1.0)
        );
        assert!(parse_probe("width=1920\nheight=1080\nduration=0").is_err());
    }
}
