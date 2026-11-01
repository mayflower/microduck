//! YAMNet features and bounded online beat analysis. No capture or motor I/O.
use anyhow::{Result, ensure};
use ort::{session::Session, value::Tensor};
use rustfft::{Fft, FftPlanner, num_complex::Complex32};
use std::{collections::VecDeque, path::Path, sync::Arc, time::Instant};

#[derive(Clone, Debug)]
pub struct MusicFrame {
    pub at: Instant,
    pub music: bool,
    pub bpm: f64,
    pub beat: f64,
    pub locked: bool,
}
pub type LatestMusic = Arc<arc_swap::ArcSwapOption<MusicFrame>>;

/// TF YAMNet v1: periodic Hann, magnitude spectrum, HTK mel, time-major patches.
pub struct Features {
    fft: Arc<dyn Fft<f32>>,
    filters: Vec<Vec<(usize, f32)>>,
}
impl Default for Features {
    fn default() -> Self {
        Self::new()
    }
}
impl Features {
    pub fn new() -> Self {
        let mel = |hz: f32| 1127.0 * (1.0 + hz / 700.0).ln();
        let lo = mel(125.0);
        let hi = mel(7500.0);
        let filters = (0..64)
            .map(|m| {
                (1..257)
                    .filter_map(|k| {
                        let x = mel(k as f32 * 16000.0 / 512.0);
                        let a = lo + (hi - lo) * m as f32 / 65.0;
                        let b = lo + (hi - lo) * (m + 1) as f32 / 65.0;
                        let c = lo + (hi - lo) * (m + 2) as f32 / 65.0;
                        let w = ((x - a) / (b - a)).min((c - x) / (c - b)).max(0.0);
                        (w > 0.0).then_some((k, w))
                    })
                    .collect()
            })
            .collect();
        Self {
            fft: FftPlanner::new().plan_fft_forward(512),
            filters,
        }
    }
    pub fn extract(&self, samples: &[f32]) -> Vec<f32> {
        assert!(samples.len() >= 15600);
        let mut out = Vec::with_capacity(96 * 64);
        let mut buf = vec![Complex32::default(); 512];
        for frame in 0..96 {
            for k in 0..512 {
                buf[k] = Complex32::new(
                    if k < 400 {
                        samples[frame * 160 + k]
                            * (0.5 - 0.5 * (std::f32::consts::TAU * k as f32 / 400.0).cos())
                    } else {
                        0.0
                    },
                    0.0,
                );
            }
            self.fft.process(&mut buf);
            for band in &self.filters {
                out.push((band.iter().map(|(k, w)| buf[*k].norm() * w).sum::<f32>() + 0.001).ln());
            }
        }
        out
    }
}

pub struct Analyzer {
    session: Session,
    features: Features,
    samples: VecDeque<f32>,
    envelope: VecDeque<f64>,
    partial: VecDeque<f32>,
    previous: Vec<f32>,
    fft: Arc<dyn Fft<f32>>,
    count: u64,
    next_infer: u64,
    music_score: f32,
    music_hits: u32,
    music: bool,
}
impl Analyzer {
    pub fn new(path: &Path) -> Result<Self> {
        let session = Session::builder()?
            .with_intra_threads(1)?
            .with_inter_threads(1)?
            .commit_from_file(path)?;
        Ok(Self {
            session,
            features: Features::new(),
            samples: VecDeque::new(),
            envelope: VecDeque::new(),
            partial: VecDeque::new(),
            previous: vec![0.0; 257],
            fft: FftPlanner::new().plan_fft_forward(512),
            count: 0,
            next_infer: 15600,
            music_score: 0.0,
            music_hits: 0,
            music: false,
        })
    }
    pub fn music_score(&self) -> f32 {
        self.music_score
    }
    pub fn reset(&mut self) {
        self.samples.clear();
        self.envelope.clear();
        self.partial.clear();
        self.previous.fill(0.0);
        self.count = 0;
        self.next_infer = 15600;
        self.music_score = 0.0;
        self.music_hits = 0;
        self.music = false;
    }
    pub fn push(&mut self, samples: &[f32], now: Instant) -> Result<MusicFrame> {
        ensure!(
            samples.iter().all(|v| v.is_finite()),
            "nonfinite microphone data"
        );
        for &x in samples {
            self.samples.push_back(x);
            if self.samples.len() > 15600 {
                self.samples.pop_front();
            }
            self.partial.push_back(x);
            if self.partial.len() > 512 {
                self.partial.pop_front();
            }
            self.count += 1;
            if self.partial.len() == 512 && self.count.is_multiple_of(160) {
                let mut bins: Vec<_> = self
                    .partial
                    .iter()
                    .enumerate()
                    .map(|(i, v)| {
                        Complex32::new(
                            v * (0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / 512.0).cos()),
                            0.0,
                        )
                    })
                    .collect();
                self.fft.process(&mut bins);
                let mut flux = 0.0;
                for (i, b) in bins.iter().take(257).enumerate() {
                    let v = b.norm().ln_1p();
                    flux += (v - self.previous[i]).max(0.0) as f64;
                    self.previous[i] = v;
                }
                self.envelope.push_back(flux);
                if self.envelope.len() > 800 {
                    self.envelope.pop_front();
                }
            }
        }
        if self.count >= self.next_infer && self.samples.len() == 15600 {
            let data: Vec<_> = self.samples.iter().copied().collect();
            let mel = self.features.extract(&data);
            let outputs = self
                .session
                .run(ort::inputs![Tensor::from_array(([1usize, 96, 64], mel))?])?;
            let (_, scores) = outputs[0].try_extract_tensor::<f32>()?;
            ensure!(
                scores.len() == 521 && scores.iter().all(|x| x.is_finite()),
                "invalid YAMNet output"
            );
            // AudioSet class map shipped by the exporter: Music is index 132.
            self.music_score = scores[132];
            self.next_infer = self.count + 7680;
            if self.music_score >= 0.6 {
                self.music_hits += 1;
            } else {
                self.music_hits = 0;
            }
            if self.music_hits >= 6 {
                self.music = true;
            }
            if self.music_score < 0.6 {
                self.music = false;
            }
        }
        let (bpm, beat, confidence) = track(&self.envelope);
        let beat = (beat + (self.count % 160) as f64 / 16000.0 * bpm / 60.0).rem_euclid(1.0);
        Ok(MusicFrame {
            at: now,
            music: self.music,
            bpm,
            beat,
            locked: self.music && confidence >= 0.55 && self.envelope.len() >= 390,
        })
    }
}
/// Autocorrelation selects a periodic pulse; recent onset positions establish its phase.
fn track(e: &VecDeque<f64>) -> (f64, f64, f64) {
    if e.len() < 390 {
        return (0.0, 0.0, 0.0);
    }
    let mean = e.iter().sum::<f64>() / e.len() as f64;
    let x: Vec<_> = e.iter().map(|v| (v - mean).max(0.0)).collect();
    let mut corr = [0.0; 88];
    for lag in 36..=87 {
        let mut dot = 0.0;
        let mut aa = 0.0;
        let mut bb = 0.0;
        for i in lag..x.len() {
            dot += x[i] * x[i - lag];
            aa += x[i] * x[i];
            bb += x[i - lag] * x[i - lag];
        }
        corr[lag] = dot / (aa * bb).sqrt().max(1e-12);
    }
    let peaks: Vec<_> = (37..=86)
        .filter(|&lag| corr[lag] >= corr[lag - 1] && corr[lag] > corr[lag + 1])
        .collect();
    let Some(best_lag) = peaks
        .iter()
        .copied()
        .max_by(|&a, &b| corr[a].total_cmp(&corr[b]))
    else {
        return (0.0, 0.0, 0.0);
    };
    if corr[best_lag] <= 0.0 {
        return (0.0, 0.0, 0.0);
    }
    // Alternating kick/snare accents otherwise make every second beat win. An octave
    // correction needs independent support for that faster pulse, not just a shorter lag.
    let lag = peaks
        .iter()
        .copied()
        .filter(|&lag| {
            ((lag as f64 * 2.0 / best_lag as f64) - 1.0).abs() < 0.04
                && corr[lag] >= 0.6 * corr[best_lag]
                && corr[lag] >= 0.55
        })
        .max_by(|&a, &b| corr[a].total_cmp(&corr[b]))
        .unwrap_or(best_lag);
    let curvature = corr[lag - 1] - 2.0 * corr[lag] + corr[lag + 1];
    let offset = if curvature.abs() > 1e-9 {
        0.5 * (corr[lag - 1] - corr[lag + 1]) / curvature
    } else {
        0.0
    };
    let period = (lag as f64 + offset.clamp(-0.5, 0.5)).clamp(37.5, 6000.0 / 70.0);
    // Score recent onsets at fractional periods, so a non-integral tempo does not drift
    // against the feature hop. A narrow peak preserves phase at the strongest pulse.
    let phase = (0..lag)
        .max_by(|a, b| {
            let score = |p: usize| {
                (0..8)
                    .map(|beat| {
                        let age = p as f64 + beat as f64 * period;
                        let i = age.round() as usize;
                        if i < x.len() { x[x.len() - 1 - i] } else { 0.0 }
                    })
                    .sum::<f64>()
            };
            score(*a).total_cmp(&score(*b))
        })
        .unwrap_or(0);
    (6000.0 / period, phase as f64 / period, corr[lag])
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires ONNX Runtime >= 1.23; run explicitly with ORT_DYLIB_PATH"]
    fn exported_model_matches_tensorflow_reference() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("models/music_yamnet.onnx");
        let mut a = Analyzer::new(&path).unwrap();
        let bytes = include_bytes!("../tests/data/yamnet-tone-mel.f32");
        let features: Vec<_> = bytes
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
            .collect();
        let output = a
            .session
            .run(ort::inputs![
                Tensor::from_array(([1usize, 96, 64], features)).unwrap()
            ])
            .unwrap();
        let (_, actual) = output[0].try_extract_tensor::<f32>().unwrap();
        let expected = include_bytes!("../tests/data/yamnet-tone-scores.f32");
        let error = actual
            .iter()
            .zip(expected.chunks_exact(4))
            .map(|(a, b)| (*a - f32::from_le_bytes(b.try_into().unwrap())).abs())
            .fold(0.0f32, f32::max);
        assert!(error < 1e-5, "TensorFlow output mismatch: {error}");
    }
    #[test]
    fn features_match_tensorflow_reference() {
        let samples: Vec<_> = (0..15600)
            .map(|i| {
                let t = i as f64 / 16000.0;
                let x = 0.15 * (std::f64::consts::TAU * 440.0 * t).sin()
                    + 0.08 * (std::f64::consts::TAU * 1234.0 * t).sin()
                    + if i % 4000 < 16 { 0.2 } else { 0.0 };
                (x * 32767.0) as i16 as f32 / 32768.0
            })
            .collect();
        let actual = Features::new().extract(&samples);
        let expected = include_bytes!("../tests/data/yamnet-tone-mel.f32");
        let error = actual
            .iter()
            .zip(expected.chunks_exact(4))
            .map(|(a, b)| (*a - f32::from_le_bytes(b.try_into().unwrap())).abs())
            .fold(0.0f32, f32::max);
        assert!(error < 0.003, "TensorFlow feature mismatch: {error}");
    }
    #[test]
    fn class_map_music_index_is_pinned() {
        assert!(
            include_str!("../models/yamnet_class_map.csv")
                .lines()
                .any(|l| l == "132,/m/04rlf,Music")
        );
    }
    #[test]
    fn alternating_accents_do_not_halve_the_tempo() {
        let e = (0..800)
            .map(|i| {
                if i % 80 == 0 {
                    10.0
                } else if i % 40 == 0 {
                    5.0
                } else {
                    0.0
                }
            })
            .collect();
        let (b, _, c) = track(&e);
        assert!((b - 150.0).abs() < 0.1);
        assert!(c > 0.55);
    }
    #[test]
    fn silence_is_not_a_beat() {
        assert_eq!(track(&VecDeque::from(vec![0.0; 800])), (0.0, 0.0, 0.0));
    }
    #[test]
    fn periodic_pulses_lock() {
        let e = (0..800)
            .map(|i| if i % 50 == 0 { 10.0 } else { 0.0 })
            .collect();
        let (b, _, c) = track(&e);
        assert!((b - 120.0).abs() < 0.1);
        assert!(c > 0.99);
    }
    #[test]
    fn yamnet_silence_features() {
        let f = Features::new().extract(&vec![0.0; 15600]);
        assert_eq!(f.len(), 6144);
        assert!(f.iter().all(|v| (*v - 0.001f32.ln()).abs() < 1e-5));
    }
}
