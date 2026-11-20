use std::{
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use core_engine::{Result, audio::AudioMeter};
use wasapi::{DeviceEnumerator, Direction, SampleType, StreamMode, WasapiError, WaveFormat};

const IDLE: u8 = 0;
const CAPTURE: u8 = 1;
const QUIT: u8 = 2;
const SAMPLE_RATE: u32 = 48_000;
const DEVICE_CHECK: Duration = Duration::from_secs(2);

pub struct PlaybackAudio {
    state: Arc<AtomicU8>,
    worker: Option<JoinHandle<()>>,
    meter: AudioMeter,
}

impl PlaybackAudio {
    pub fn new() -> Result<Self> {
        let state = Arc::new(AtomicU8::new(IDLE));
        let meter = AudioMeter::default();
        meter.set_active(false);
        let worker_state = state.clone();
        let worker_meter = meter.clone();
        let worker = thread::Builder::new()
            .name("wallp-playback".into())
            .spawn(move || capture_worker(&worker_state, &worker_meter))?;
        Ok(Self {
            state,
            worker: Some(worker),
            meter,
        })
    }

    pub fn set_active(&self, active: bool) {
        let state = if active { CAPTURE } else { IDLE };
        self.meter.set_active(active);
        if self.state.swap(state, Ordering::Relaxed) != state
            && let Some(worker) = &self.worker
        {
            worker.thread().unpark();
        }
    }

    pub fn bands(&self) -> [f32; 16] {
        self.meter.bands()
    }
}

impl Drop for PlaybackAudio {
    fn drop(&mut self) {
        self.state.store(QUIT, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            worker.thread().unpark();
            let _ = worker.join();
        }
    }
}

fn capture_worker(state: &AtomicU8, meter: &AudioMeter) {
    if let Err(error) = wasapi::initialize_mta().ok() {
        eprintln!("wallp: Windows playback audio initialization failed: {error}");
        return;
    }
    let mut last_error = None;
    loop {
        match state.load(Ordering::Relaxed) {
            QUIT => break,
            IDLE => thread::park(),
            _ => match listen(state, meter) {
                Ok(()) => last_error = None,
                Err(error) => {
                    let message = error.to_string();
                    if last_error.as_ref() != Some(&message) {
                        eprintln!("wallp: Windows playback audio: {message}");
                        last_error = Some(message);
                    }
                    thread::park_timeout(DEVICE_CHECK);
                }
            },
        }
    }
    wasapi::deinitialize();
}

fn listen(state: &AtomicU8, meter: &AudioMeter) -> Result<()> {
    let enumerator = DeviceEnumerator::new()?;
    // Capture direction on a render endpoint is WASAPI loopback.
    let device = enumerator.get_default_device(Direction::Render)?;
    let device_id = device.get_id()?;
    let mut stream = device.get_iaudioclient()?;
    let format = WaveFormat::new(32, 32, SampleType::Float, SAMPLE_RATE as usize, 2, None);
    let (period, _) = stream.get_device_period()?;
    stream.initialize_client(
        &format,
        Direction::Capture,
        StreamMode::EventsShared {
            autoconvert: true,
            buffer_duration_hns: period,
        },
        None,
    )?;
    let event = stream.set_get_eventhandle()?;
    let capture = stream.get_audiocaptureclient()?;
    let capacity = stream.get_buffer_size()? as usize;
    let mut bytes = vec![0; capacity * 8];
    let mut mono = Vec::with_capacity(capacity);
    stream.start_stream()?;
    let mut next_device_check = Instant::now() + DEVICE_CHECK;
    while state.load(Ordering::Relaxed) == CAPTURE {
        match event.wait_for_event(200) {
            Ok(()) | Err(WasapiError::EventTimeout) => {}
            Err(error) => return Err(error.into()),
        }
        while state.load(Ordering::Relaxed) == CAPTURE
            && capture
                .get_next_packet_size()?
                .is_some_and(|frames| frames != 0)
        {
            let (frames, _) = capture.read_from_device(&mut bytes)?;
            mono.clear();
            let (stereo, _) = bytes[..frames as usize * 8].as_chunks::<8>();
            for frame in stereo {
                let ([left, right], _) = frame.as_chunks::<4>() else {
                    unreachable!("stereo frame has two samples");
                };
                mono.push((f32::from_le_bytes(*left) + f32::from_le_bytes(*right)) * 0.5);
            }
            meter.push_mono(&mono, SAMPLE_RATE);
        }
        if Instant::now() >= next_device_check {
            if enumerator.get_default_device(Direction::Render)?.get_id()? != device_id {
                break;
            }
            next_device_check = Instant::now() + DEVICE_CHECK;
        }
    }
    stream.stop_stream()?;
    Ok(())
}
