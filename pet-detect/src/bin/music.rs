//! Offline replay of the exact onboard analyzer, with virtual capture timestamps.
use anyhow::{Result, ensure};
use clap::Parser;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
#[derive(Parser)]
struct Args {
    /// Mono 16 kHz, signed 16-bit PCM WAV.
    wav: PathBuf,
    #[arg(long, default_value = "pet-detect/models/music_yamnet.onnx")]
    model: PathBuf,
    /// Dump the first YAMNet feature patch as little-endian float32.
    #[arg(long)]
    features: Option<PathBuf>,
}
fn main() -> Result<()> {
    let args = Args::parse();
    let mut wav = hound::WavReader::open(args.wav)?;
    let spec = wav.spec();
    ensure!(
        spec.channels == 1
            && spec.sample_rate == 16000
            && spec.bits_per_sample == 16
            && spec.sample_format == hound::SampleFormat::Int,
        "expected mono 16kHz PCM16"
    );
    let pcm: Vec<_> = wav
        .samples::<i16>()
        .collect::<std::result::Result<_, _>>()?;
    let samples = pet_detect::i16_to_f32(&pcm);
    if let Some(path) = args.features {
        ensure!(samples.len() >= 15600, "audio too short");
        let mel = pet_detect::music::Features::new().extract(&samples[..15600]);
        std::fs::write(
            path,
            mel.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>(),
        )?;
    }
    let mut analyzer = pet_detect::music::Analyzer::new(&args.model)?;
    let epoch = Instant::now();
    let mut count = 0;
    let mut timings = Vec::new();
    let mut first_lock = None;
    for chunk in samples.chunks(512) {
        count += chunk.len();
        let seconds = count as f64 / 16000.0;
        let before = Instant::now();
        let f = analyzer.push(chunk, epoch + Duration::from_secs_f64(seconds))?;
        let ms = before.elapsed().as_secs_f64() * 1000.0;
        timings.push(ms);
        if f.locked && first_lock.is_none() {
            first_lock = Some(seconds);
        }
        println!(
            "{}",
            serde_json::json!({"seconds":seconds,"music_score":analyzer.music_score(),"music":f.music,"locked":f.locked,"bpm":f.bpm,"beat":f.beat,"analysis_ms":ms})
        );
    }
    timings.sort_by(f64::total_cmp);
    eprintln!(
        "{}",
        serde_json::json!({"duration_s":samples.len() as f64/16000.0,"first_lock_s":first_lock,"analysis_max_ms":timings.last(),"analysis_p99_ms":timings.get(timings.len().saturating_sub(1)*99/100)})
    );
    Ok(())
}
