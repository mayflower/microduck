// SPDX-License-Identifier: GPL-3.0-or-later
//! Standalone audio utility: raw mono f32le stdin → resonance-colored f32le stdout.
use std::io::{Read, Write};

const VERSION: &str = "duck-voice-color 1 (Rubber Band 4.0.0; preset B)";
const MAX_SECONDS: usize = 30;
unsafe extern "C" {
    fn duck_voice_color(
        input: *const f32,
        count: usize,
        output: *mut f32,
        rate: u32,
        formant: f64,
    ) -> i32;
}

#[derive(Clone, Copy)]
struct Settings {
    rate: u32,
    formant: f64,
    loudness: f64,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            rate: 24_000,
            formant: 1.5,
            loudness: -21.0,
        }
    }
}

fn color(input: &[f32], settings: Settings) -> Result<Vec<f32>, String> {
    if input.is_empty()
        || input.len() > settings.rate as usize * MAX_SECONDS
        || input.iter().any(|v| !v.is_finite())
    {
        return Err("expected finite, nonempty PCM of at most 30 seconds".into());
    }
    let mut output = vec![0.0; input.len()];
    // SAFETY: both buffers contain count f32s, do not alias, and stay live throughout the call.
    let result = unsafe {
        duck_voice_color(
            input.as_ptr(),
            input.len(),
            output.as_mut_ptr(),
            settings.rate,
            settings.formant,
        )
    };
    if result != 0 || output.iter().any(|s| !s.is_finite()) {
        return Err(format!("resonance processing failed ({result})"));
    }
    master(&mut output, settings)?;
    Ok(output)
}

fn master(pcm: &mut [f32], settings: Settings) -> Result<(), String> {
    let mut meter =
        ebur128::EbuR128::new(1, settings.rate, ebur128::Mode::I).map_err(|e| e.to_string())?;
    meter.add_frames_f32(pcm).map_err(|e| e.to_string())?;
    let loudness = meter.loudness_global().map_err(|e| e.to_string())?;
    // No large boost for silence/very short utterances. Static gain preserves syllable dynamics;
    // reducing the whole buffer for peak headroom adds no distortion or modulation.
    let requested = if loudness.is_finite() {
        10.0f64.powf((settings.loudness - loudness).clamp(-12.0, 12.0) / 20.0)
    } else {
        1.0
    };
    let peak = pcm
        .iter()
        .map(|s| f64::from(s.abs()))
        .fold(0.0f64, f64::max);
    let gain = if peak > 0.0 {
        requested.min(0.8 / peak)
    } else {
        1.0
    } as f32;
    for s in pcm {
        *s *= gain;
    }
    Ok(())
}

fn run() -> Result<(), String> {
    let mut settings = Settings::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--version" => { println!("{VERSION}"); return Ok(()); }
            "--licenses" => {
                println!("{}\n{}\n{}", include_str!("../COPYING"),
                    include_str!("../vendor/rubberband/COPYING"), concat!(include_str!("../NOTICE"), "\n", include_str!("../vendor/LICENSE-ebur128")));
                return Ok(());
            }
            "--sample-rate" => settings.rate = args.next().ok_or("missing sample rate")?.parse().map_err(|_| "invalid sample rate")?,
            "--formant" => settings.formant = args.next().ok_or("missing formant ratio")?.parse().map_err(|_| "invalid formant ratio")?,
            "--loudness" => settings.loudness = args.next().ok_or("missing loudness")?.parse().map_err(|_| "invalid loudness")?,
            _ => return Err("usage: duck-voice-color [--sample-rate HZ] [--formant FACTOR] [--loudness LUFS] [--version|--licenses]".into()),
        }
    }
    if !(8_000..=96_000).contains(&settings.rate)
        || !(0.5..=2.0).contains(&settings.formant)
        || !(-35.0..=-14.0).contains(&settings.loudness)
    {
        return Err("unsupported sample rate, formant ratio or loudness".into());
    }
    let max = settings.rate as usize * MAX_SECONDS * 4;
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(max as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.is_empty() || bytes.len() > max || !bytes.len().is_multiple_of(4) {
        return Err("expected nonempty raw mono f32le PCM of at most 30 seconds".into());
    }
    let pcm: Vec<f32> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|s| f32::from_le_bytes(*s))
        .collect();
    let output = color(&pcm, settings)?;
    let mut stdout = std::io::BufWriter::new(std::io::stdout().lock());
    for sample in output {
        stdout
            .write_all(&sample.to_le_bytes())
            .map_err(|e| e.to_string())?;
    }
    stdout.flush().map_err(|e| e.to_string())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("duck-voice-color: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn processing_preserves_length_and_silence_and_refuses_invalid_pcm() {
        let settings = Settings::default();
        let silent = color(&vec![0.0; 24_000], settings).unwrap();
        assert_eq!(silent.len(), 24_000);
        assert!(silent.iter().all(|s| *s == 0.0));
        assert!(color(&[], settings).is_err());
        assert!(color(&[f32::NAN], settings).is_err());
        let pcm: Vec<_> = (0..24_000)
            .map(|i| (i as f32 * 200.0 * std::f32::consts::TAU / 24_000.0).sin() * 0.2)
            .collect();
        let out = color(&pcm, settings).unwrap();
        assert_eq!(out.len(), pcm.len());
        assert!(out.iter().all(|s| s.is_finite() && s.abs() <= 0.80001));
    }

    #[test]
    fn nasal_color_changes_vowel_resonance_without_changing_fundamental_or_duration() {
        let settings = Settings::default();
        let tau = std::f64::consts::TAU;
        let input: Vec<f32> = (0..24_000)
            .map(|i| {
                (1..=45)
                    .map(|k| {
                        let f = k as f64 * 200.0;
                        let envelope = 0.01 / k as f64
                            + (-((f - 600.0) / 100.0).powi(2)).exp()
                            + 0.7 * (-((f - 1700.0) / 180.0).powi(2)).exp()
                            + 0.3 * (-((f - 2800.0) / 180.0).powi(2)).exp();
                        0.08 * envelope * (tau * f * i as f64 / 24_000.0).sin()
                    })
                    .sum::<f64>() as f32
            })
            .collect();
        let output = color(&input, settings).unwrap();
        assert_eq!(input.len(), output.len());
        let pitch_lag = |audio: &[f32]| {
            (80..160)
                .max_by(|a, b| {
                    let score = |lag: usize| {
                        (5000..15_000)
                            .map(|i| f64::from(audio[i]) * f64::from(audio[i - lag]))
                            .sum::<f64>()
                    };
                    score(*a).total_cmp(&score(*b))
                })
                .unwrap()
        };
        assert_eq!(
            pitch_lag(&output),
            120,
            "fundamental moved away from 200 Hz"
        );
        let centroid = |audio: &[f32]| {
            let mut weighted = 0.0;
            let mut total = 0.0;
            for k in 1..=45 {
                let f = k as f64 * 200.0;
                let mut re = 0.0;
                let mut im = 0.0;
                for (i, sample) in audio[5000..9800].iter().enumerate() {
                    let phase = tau * f * i as f64 / 24_000.0;
                    re += f64::from(*sample) * phase.cos();
                    im += f64::from(*sample) * phase.sin();
                }
                let power = re * re + im * im;
                weighted += f * power;
                total += power;
            }
            weighted / total
        };
        assert!(
            centroid(&output) > centroid(&input) * 1.15,
            "resonance bypassed at unity pitch"
        );
    }
}
