use std::{
    ops::Range,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering::Relaxed},
    },
    time::{Duration, Instant},
};

use rustfft::{Fft, FftPlanner, num_complex::Complex32};

pub const BAND_COUNT: usize = 16;
const FFT_SIZE: usize = 2048;
const BAND_EDGES: [f32; BAND_COUNT + 1] = [
    30.0, 55.0, 80.0, 110.0, 160.0, 230.0, 330.0, 470.0, 680.0, 970.0, 1400.0, 2000.0, 2850.0,
    4100.0, 5800.0, 8300.0, 16000.0,
];
const HOLD: Duration = Duration::from_millis(120);
const DECAY: Duration = Duration::from_millis(280);

#[derive(Clone)]
pub struct AudioMeter(Arc<MeterState>);

struct MeterState {
    active: AtomicBool,
    analysis: Mutex<Analysis>,
}

impl Default for AudioMeter {
    fn default() -> Self {
        Self(Arc::new(MeterState {
            active: AtomicBool::new(true),
            analysis: Mutex::new(Analysis::default()),
        }))
    }
}

impl AudioMeter {
    pub fn set_active(&self, active: bool) {
        let was_active = self.0.active.swap(active, Relaxed);
        if was_active
            && !active
            && let Ok(mut analysis) = self.0.analysis.lock()
        {
            analysis.written = 0;
            analysis.updated = None;
            analysis.bands = [0.0; BAND_COUNT];
        }
    }

    pub fn push_mono(&self, samples: &[f32], sample_rate: u32) {
        if sample_rate != 0
            && self.0.active.load(Relaxed)
            && let Ok(mut analysis) = self.0.analysis.lock()
            && self.0.active.load(Relaxed)
        {
            analysis.push(samples, sample_rate);
        }
    }

    pub fn bands(&self) -> [f32; BAND_COUNT] {
        let Ok(analysis) = self.0.analysis.lock() else {
            return [0.0; BAND_COUNT];
        };
        let gain = analysis.updated.map_or(0.0, |updated| {
            let elapsed = updated.elapsed().saturating_sub(HOLD);
            (1.0 - elapsed.as_secs_f32() / DECAY.as_secs_f32()).max(0.0)
        });
        analysis.bands.map(|band| band * gain)
    }
}

struct Analysis {
    fft: Arc<dyn Fft<f32>>,
    scratch: Vec<Complex32>,
    block: Vec<Complex32>,
    window: [f32; FFT_SIZE],
    written: usize,
    sample_rate: u32,
    bins: [Range<usize>; BAND_COUNT],
    bands: [f32; BAND_COUNT],
    updated: Option<Instant>,
}

impl Default for Analysis {
    fn default() -> Self {
        let fft = FftPlanner::new().plan_fft_forward(FFT_SIZE);
        Self {
            scratch: vec![Complex32::default(); fft.get_inplace_scratch_len()],
            fft,
            block: vec![Complex32::default(); FFT_SIZE],
            window: std::array::from_fn(|index| {
                0.5 - 0.5 * (std::f32::consts::TAU * index as f32 / FFT_SIZE as f32).cos()
            }),
            written: 0,
            sample_rate: 0,
            bins: std::array::from_fn(|_| 0..0),
            bands: [0.0; BAND_COUNT],
            updated: None,
        }
    }
}

impl Analysis {
    fn push(&mut self, samples: &[f32], sample_rate: u32) {
        if self.sample_rate != sample_rate {
            self.sample_rate = sample_rate;
            self.bins = std::array::from_fn(|index| {
                let bin = |hz: f32| {
                    ((hz * FFT_SIZE as f32 / sample_rate as f32).ceil() as usize)
                        .clamp(1, FFT_SIZE / 2 + 1)
                };
                bin(BAND_EDGES[index])..bin(BAND_EDGES[index + 1])
            });
            self.bands = [0.0; BAND_COUNT];
            self.written = 0;
        }
        for &sample in samples {
            let sample = if sample.is_finite() {
                sample.clamp(-1.0, 1.0)
            } else {
                0.0
            };
            self.block[self.written] = Complex32::new(sample * self.window[self.written], 0.0);
            self.written += 1;
            if self.written == FFT_SIZE {
                self.analyze();
                self.written = 0;
            }
        }
    }

    fn analyze(&mut self) {
        if self
            .updated
            .is_some_and(|updated| updated.elapsed() >= HOLD + DECAY)
        {
            self.bands = [0.0; BAND_COUNT];
        }
        self.fft
            .process_with_scratch(&mut self.block, &mut self.scratch);
        for (band, bins) in self.bands.iter_mut().zip(&self.bins) {
            let peak = self.block[bins.clone()]
                .iter()
                .map(Complex32::norm_sqr)
                .fold(0.0, f32::max)
                .sqrt();
            // Compensate for the Hann window and map a 60 dB range onto 0..1.
            let amplitude = peak * (4.0 / FFT_SIZE as f32);
            let level = ((amplitude.max(0.001).log10() + 3.0) / 3.0).clamp(0.0, 1.0);
            let weight = if level > *band { 0.65 } else { 0.25 };
            *band += (level - *band) * weight;
        }
        self.updated = Some(Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tone_lights_its_band_and_silence_clears_it() {
        let meter = AudioMeter::default();
        let samples: Vec<f32> = (0..FFT_SIZE)
            .map(|index| (std::f32::consts::TAU * 440.0 * index as f32 / 48_000.0).sin())
            .collect();
        meter.push_mono(&samples[..100], 48_000);
        assert_eq!(meter.bands(), [0.0; BAND_COUNT]);
        meter.push_mono(&samples[100..], 48_000);
        let bands = meter.bands();
        assert!(bands[6] > 0.5);
        assert!(bands[6] > bands[5] + 0.2);
        assert!(bands[6] > bands[7] + 0.2);
        meter.set_active(false);
        meter.push_mono(&samples, 48_000);
        assert_eq!(meter.bands(), [0.0; BAND_COUNT]);
        meter.set_active(true);
        meter.push_mono(&samples, 48_000);
        assert!(meter.bands()[6] > 0.5);
        for _ in 0..16 {
            meter.push_mono(&[0.0; FFT_SIZE], 48_000);
        }
        assert!(meter.bands().iter().all(|value| *value < 0.01));
    }
}
