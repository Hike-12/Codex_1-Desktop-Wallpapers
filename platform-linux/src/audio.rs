use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering::Relaxed},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use core_engine::{Result, audio::AudioMeter};
use pulse::callbacks::ListResult;
use pulse::{
    context::{self, Context, introspect::SinkInfo},
    def::BufferAttr,
    mainloop::standard::{IterateResult, Mainloop},
    operation,
    sample::{Format, Spec},
    stream::{self, PeekResult, Stream},
};

const IDLE: u8 = 0;
const CAPTURE: u8 = 1;
const QUIT: u8 = 2;
const SAMPLE_RATE: u32 = 48_000;
const POLL: Duration = Duration::from_millis(10);
const RETRY: Duration = Duration::from_secs(2);
const DEVICE_CHECK: Duration = Duration::from_secs(4);

pub struct Playback {
    meter: AudioMeter,
    state: Arc<AtomicU8>,
    worker: Option<JoinHandle<()>>,
}

impl Playback {
    pub fn start(enabled: bool) -> Option<Self> {
        if !enabled {
            return None;
        }
        let meter = AudioMeter::default();
        meter.set_active(false);
        let state = Arc::new(AtomicU8::new(IDLE));
        let (worker_state, worker_meter) = (state.clone(), meter.clone());
        match thread::Builder::new()
            .name("wallp-playback".into())
            .spawn(move || worker(&worker_state, &worker_meter))
        {
            Ok(worker) => Some(Self {
                meter,
                state,
                worker: Some(worker),
            }),
            Err(error) => {
                eprintln!("wallp: playback capture unavailable: {error}");
                None
            }
        }
    }

    pub fn bands(&self) -> [f32; 16] {
        self.meter.bands()
    }

    pub fn set_active(&self, active: bool) {
        let next = if active { CAPTURE } else { IDLE };
        self.meter.set_active(active);
        if self.state.swap(next, Relaxed) != next
            && let Some(worker) = &self.worker
        {
            worker.thread().unpark();
        }
    }
}

impl Drop for Playback {
    fn drop(&mut self) {
        self.state.store(QUIT, Relaxed);
        if let Some(worker) = self.worker.take() {
            worker.thread().unpark();
            let _ = worker.join();
        }
    }
}

fn worker(state: &AtomicU8, meter: &AudioMeter) {
    let mut last_error = None;
    loop {
        match state.load(Relaxed) {
            QUIT => return,
            IDLE => thread::park(),
            _ => match record(state, meter) {
                Ok(()) => last_error = None,
                Err(error) => {
                    let message = error.to_string();
                    if last_error.as_ref() != Some(&message) {
                        eprintln!("wallp: Linux playback audio: {message}");
                        last_error = Some(message);
                    }
                    thread::park_timeout(RETRY);
                }
            },
        }
    }
}

fn record(state: &AtomicU8, meter: &AudioMeter) -> Result<()> {
    let mut event_loop = Mainloop::new().ok_or("Could not create a PulseAudio mainloop")?;
    let mut context = Context::new(&event_loop, "Desktop Wallpapers")
        .ok_or("Could not create a PulseAudio context")?;
    context.connect(None, context::FlagSet::NOAUTOSPAWN, None)?;
    if !wait_until(&mut event_loop, state, || match context.get_state() {
        context::State::Ready => Ok(true),
        context::State::Failed | context::State::Terminated => {
            Err("Could not connect to PulseAudio or PipeWire-Pulse".into())
        }
        _ => Ok(false),
    })? {
        return Ok(());
    }
    let Some(monitor) = default_monitor(&mut event_loop, &context, state)? else {
        return Ok(());
    };
    let spec = Spec {
        format: Format::F32le,
        channels: 1,
        rate: SAMPLE_RATE,
    };
    let mut stream = Stream::new(&mut context, "Wallpaper playback", &spec, None)
        .ok_or("Could not create a PulseAudio playback monitor stream")?;
    let buffer = BufferAttr {
        maxlength: u32::MAX,
        tlength: u32::MAX,
        prebuf: u32::MAX,
        minreq: u32::MAX,
        fragsize: 4096,
    };
    // Pin the explicitly resolved output monitor. If it disappears, never let
    // the audio server rescue this stream by moving it to an input device.
    stream.connect_record(Some(&monitor), Some(&buffer), stream::FlagSet::DONT_MOVE)?;
    if !wait_until(&mut event_loop, state, || match stream.get_state() {
        stream::State::Ready => Ok(true),
        stream::State::Failed | stream::State::Terminated => {
            Err("Could not open the default output's monitor".into())
        }
        _ => Ok(false),
    })? {
        return Ok(());
    }
    if stream.get_device_name().as_deref() != Some(monitor.as_str()) {
        return Err("The audio server did not attach the selected playback monitor".into());
    }

    let mut samples = Vec::with_capacity(1024);
    let mut next_check = Instant::now() + DEVICE_CHECK;
    while state.load(Relaxed) == CAPTURE {
        dispatch(&mut event_loop)?;
        if stream.get_state() != stream::State::Ready {
            return Err("The playback monitor disconnected".into());
        }
        loop {
            match stream.peek()? {
                PeekResult::Empty => break,
                PeekResult::Data(data) => {
                    samples.clear();
                    samples.extend(
                        data.as_chunks::<4>()
                            .0
                            .iter()
                            .copied()
                            .map(f32::from_le_bytes),
                    );
                    meter.push_mono(&samples, SAMPLE_RATE);
                }
                PeekResult::Hole(_) => {}
            }
            stream.discard()?;
        }
        if Instant::now() >= next_check {
            if default_monitor(&mut event_loop, &context, state)?.as_ref() != Some(&monitor) {
                break;
            }
            next_check = Instant::now() + DEVICE_CHECK;
        }
        thread::park_timeout(POLL);
    }
    Ok(())
}

fn default_monitor(
    event_loop: &mut Mainloop,
    context: &Context,
    state: &AtomicU8,
) -> Result<Option<String>> {
    let sink = Rc::new(RefCell::new(None));
    let result = sink.clone();
    let op = context.introspect().get_server_info(move |info| {
        *result.borrow_mut() = info.default_sink_name.as_deref().map(str::to_owned);
    });
    if !wait_until(event_loop, state, || {
        Ok(op.get_state() != operation::State::Running)
    })? {
        return Ok(None);
    }
    let sink = sink.take().ok_or("No default PulseAudio playback output")?;
    let monitor = Rc::new(RefCell::new(None));
    let result = monitor.clone();
    let op = context.introspect().get_sink_info_by_name(
        &sink,
        move |item: ListResult<&SinkInfo<'_>>| {
            if let ListResult::Item(info) = item {
                *result.borrow_mut() = info.monitor_source_name.as_deref().map(str::to_owned);
            }
        },
    );
    if !wait_until(event_loop, state, || {
        Ok(op.get_state() != operation::State::Running)
    })? {
        return Ok(None);
    }
    monitor
        .take()
        .map(Some)
        .ok_or_else(|| "The default audio output has no playback monitor".into())
}

fn wait_until(
    event_loop: &mut Mainloop,
    state: &AtomicU8,
    ready: impl Fn() -> Result<bool>,
) -> Result<bool> {
    let deadline = Instant::now() + RETRY;
    while state.load(Relaxed) == CAPTURE {
        dispatch(event_loop)?;
        if ready()? {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Err("Timed out waiting for the playback monitor".into());
        }
        thread::park_timeout(POLL);
    }
    Ok(false)
}

fn dispatch(event_loop: &mut Mainloop) -> Result<()> {
    match event_loop.iterate(false) {
        IterateResult::Success(_) => Ok(()),
        IterateResult::Quit(_) | IterateResult::Err(_) => {
            Err("The PulseAudio connection ended".into())
        }
    }
}
