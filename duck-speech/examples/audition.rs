//! Generate auditable test audio with the actual native text frontend and speech engine.
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::{path::PathBuf, time::Instant};
#[derive(Deserialize)]
struct Case {
    id: String,
    text: String,
}
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    ensure!(args.len() == 3, "usage: audition MODEL CASES.json OUT_DIR");
    let cases: Vec<Case> = serde_json::from_slice(&std::fs::read(&args[1])?)?;
    let out = PathBuf::from(&args[2]);
    std::fs::create_dir_all(&out)?;
    let mut engine = duck_speech::Engine::new(&PathBuf::from(&args[0]))?;
    let mut results = Vec::new();
    for case in cases {
        ensure!(
            case.id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "invalid case filename"
        );
        let phones = engine.phonemes(&case.text)?;
        let start = Instant::now();
        let audio = engine
            .synthesize(&case.text, &Default::default())
            .with_context(|| case.id.clone())?;
        let elapsed = start.elapsed().as_secs_f64();
        let file = out.join(format!("{}.wav", case.id));
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: duck_speech::SAMPLE_RATE,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(file, spec)?;
        for sample in &audio.pcm {
            writer.write_sample((sample.clamp(-1.0, 1.0) * 32767.0).round() as i16)?;
        }
        writer.finalize()?;
        results.push(serde_json::json!({"id":case.id,"text":case.text,"phonemes":phones,"synthesis_s":elapsed,"samples":audio.pcm.len(),"audio_s":audio.pcm.len() as f64/f64::from(duck_speech::SAMPLE_RATE)}));
    }
    std::fs::write(
        out.join("native.json"),
        serde_json::to_vec_pretty(&results)?,
    )?;
    println!("Generated {} native Heart samples", results.len());
    Ok(())
}
