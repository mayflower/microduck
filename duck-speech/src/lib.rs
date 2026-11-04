//! On-demand speech, entirely local: English text → Misaki phonemes → Paradee Heart PCM.
//! [The design](../../docs/design/speech.md) owns synthesis and integration contracts.
mod color;
use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use anyhow::{Context, Result, ensure};
use arc_swap::ArcSwapOption;
use loqui_g2p::{Dialect, G2p, OovFallback};
use ort::{
    session::{RunOptions, Session, builder::GraphOptimizationLevel},
    value::Tensor,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// Redistribution notices for the model and embedded native phonemizer.
pub const LICENSES: &str = concat!(
    "Paradee v1.2 (Sahil Mahendrakar):\n",
    include_str!("../assets/LICENSE-Paradee"),
    "\nNative frontend, loqui-g2p 0.4.0:\n",
    include_str!("../assets/LICENSE-Loqui"),
    "\nUpstream loqui notices (only its G2P component is linked here):\n",
    include_str!("../assets/NOTICE-Loqui"),
    "\nSeparate voice-color utility (not linked into this library):\n",
    include_str!("../../duck-voice-color/NOTICE"),
    "\nVoice-color utility license:\n",
    include_str!("../../duck-voice-color/COPYING")
);

pub const SAMPLE_RATE: u32 = 24_000;
pub const VOICE: &str = "af_heart";
pub const DEFAULT_MODEL_PATH: &str = "/var/lib/robot/speech/paradee-v1.2-int8.onnx";
pub const MODEL_FILE: &str = "paradee-v1.2-int8.onnx";
pub const MODEL_REVISION: &str = "ed2934d4df8a4fd891e29584c5e1104649409765";
pub const MODEL_SHA256: &str = "a6989796692ac173597e15d405c971989202d5c73cfad65f22e90bd7b6c97da1";
pub const MODEL_BYTES: u64 = 8_462_119;
pub const MODEL_URL: &str = "https://huggingface.co/sahilmahendrakar/Paradee-8M-v1.2/resolve/ed2934d4df8a4fd891e29584c5e1104649409765/onnx/paradee_int8.onnx";
pub const MAX_TEXT_BYTES: usize = duck_ipc_proto::MAX_SPEECH_TEXT_BYTES;
const MAX_CHUNK_PHONEMES: usize = 256;
const MAX_SAMPLES: usize = SAMPLE_RATE as usize * 30;

/// Cancellation reaches an active ONNX run as well as the boundaries between chunks.
#[derive(Clone, Default)]
pub struct Cancellation {
    cancelled: Arc<AtomicBool>,
    run: Arc<ArcSwapOption<RunOptions>>,
}
impl Cancellation {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        if let Some(options) = self.run.load_full() {
            let _ = options.terminate();
        }
    }
    pub fn check(&self) -> Result<()> {
        ensure!(!self.cancelled.load(Ordering::Acquire), "speech cancelled");
        Ok(())
    }
}

pub fn validate_text(text: &str) -> Result<()> {
    ensure!(!text.trim().is_empty(), "speech text is empty");
    ensure!(
        text.len() <= MAX_TEXT_BYTES,
        "speech text exceeds {MAX_TEXT_BYTES} UTF-8 bytes"
    );
    ensure!(
        !text
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t')),
        "speech text contains control characters"
    );
    // English punctuation and accents are accepted; other scripts are not silently discarded.
    ensure!(
        !text
            .chars()
            .any(|c| c.is_alphabetic() && !c.is_ascii() && !matches!(c as u32, 0x00c0..=0x024f)),
        "speech supports English text only"
    );
    Ok(())
}

pub fn verify_model(path: &Path) -> Result<()> {
    ensure!(
        path.metadata()
            .with_context(|| format!(
                "speech model missing at {}; run `sounds speech install`",
                path.display()
            ))?
            .len()
            == MODEL_BYTES,
        "unexpected speech model size"
    );
    let data = std::fs::read(path)?;
    let digest: String = Sha256::digest(&data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    ensure!(digest == MODEL_SHA256, "speech model checksum mismatch");
    Ok(())
}

#[derive(Deserialize)]
struct Config {
    sample_rate: u32,
    voices: Vec<String>,
    vocab: std::collections::HashMap<char, i64>,
}

/// The model and phonemizer are loaded once by their background owner, never by a control tick.
pub struct Engine {
    session: Session,
    g2p: G2p,
    vocab: std::collections::HashMap<char, i64>,
    color: Option<color::Color>,
}
impl Engine {
    pub fn new(model: &Path) -> Result<Self> {
        Self::with_nasal(model, true)
    }

    /// Disable nasal color for an unprocessed Heart reference. The robot defaults to nasal Heart.
    pub fn with_nasal(model: &Path, nasal: bool) -> Result<Self> {
        verify_model(model)?;
        let color = nasal.then(color::Color::discover).transpose()?;
        let config: Config = serde_json::from_str(include_str!("../assets/paradee-v1.2.json"))?;
        ensure!(
            config.sample_rate == SAMPLE_RATE && config.voices.first().is_some_and(|v| v == VOICE),
            "unexpected speech configuration"
        );
        let name = std::env::var("ORT_DYLIB_PATH")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                if cfg!(target_os = "macos") {
                    "libonnxruntime.dylib".into()
                } else {
                    "libonnxruntime.so".into()
                }
            });
        // ort panics for an absent or too-old dylib. Missing speech must remain optional.
        unsafe { libloading::Library::new(&name) }
            .with_context(|| format!("cannot load {name}"))?;
        let session =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> ort::Result<Session> {
                Session::builder()?
                    .with_optimization_level(GraphOptimizationLevel::Level3)?
                    .with_intra_threads(1)?
                    .with_inter_threads(1)?
                    .with_memory_pattern(false)?
                    .with_execution_providers([ort::ep::CPU::default()
                        .with_arena_allocator(false)
                        .build()])?
                    .commit_from_file(model)
            }))
            .map_err(|_| {
                anyhow::anyhow!(
                    "ONNX Runtime initialization panicked; check the installed runtime version"
                )
            })??;
        let input_names: Vec<_> = session.inputs().iter().map(|v| v.name()).collect();
        ensure!(
            input_names == ["input_ids", "speed", "voice"],
            "unexpected speech model inputs"
        );
        ensure!(
            session.outputs().iter().any(|v| v.name() == "waveform"),
            "speech model has no waveform output"
        );
        Ok(Self {
            session,
            color,
            g2p: G2p::new(Dialect::American, OovFallback::Neural)?,
            vocab: config.vocab,
        })
    }

    pub fn phonemes(&self, text: &str) -> Result<String> {
        validate_text(text)?;
        let tokens = self.g2p.tokens(text);
        for token in &tokens {
            ensure!(
                !token.phonemes.is_empty() || !token.text.chars().any(char::is_alphanumeric),
                "cannot pronounce {:?}",
                token.text
            );
        }
        let phones = tokens
            .into_iter()
            .map(|t| t.phonemes + if t.whitespace { " " } else { "" })
            .collect::<String>();
        let phones = phones.split_whitespace().collect::<Vec<_>>().join(" ");
        ensure!(!phones.is_empty(), "no pronounceable text");
        for phone in phones.chars() {
            ensure!(
                self.vocab.contains_key(&phone),
                "unsupported speech phoneme {phone:?}"
            );
        }
        Ok(phones)
    }

    pub fn synthesize(&mut self, text: &str, cancel: &Cancellation) -> Result<Audio> {
        cancel.check()?;
        validate_text(text)?;
        let mut prepared = Vec::new();
        // Match the approved reference's sentence boundaries; never truncate token IDs.
        for sentence in sentences(text) {
            cancel.check()?;
            prepared.extend(chunks(&self.phonemes(sentence)?)?);
        }
        let mut pcm = Vec::new();
        for chunk in prepared {
            cancel.check()?;
            let mut ids = vec![0i64];
            ids.extend(chunk.chars().map(|c| self.vocab[&c]));
            ids.push(0);
            let input = Tensor::from_array(([1usize, ids.len()], ids))?;
            let speed = Tensor::from_array(([1usize], vec![1.0f32]))?;
            let voice = Tensor::from_array(([1usize], vec![0i64]))?;
            let options = Arc::new(RunOptions::new()?);
            cancel.run.store(Some(options.clone()));
            if let Err(error) = cancel.check() {
                cancel.run.store(None);
                return Err(error);
            }
            let result = self.session.run_with_options(
                ort::inputs!["input_ids" => input, "speed" => speed, "voice" => voice],
                &options,
            );
            cancel.run.store(None);
            let outputs = result?;
            cancel.check()?;
            let (_, audio) = outputs["waveform"].try_extract_tensor::<f32>()?;
            ensure!(
                !audio.is_empty() && audio.iter().all(|v| v.is_finite()),
                "invalid speech audio"
            );
            ensure!(
                pcm.len() + audio.len() <= MAX_SAMPLES,
                "speech exceeds 30 seconds"
            );
            pcm.extend_from_slice(audio);
        }
        let pcm = if let Some(color) = &self.color {
            color.apply(pcm, cancel)?
        } else {
            pcm
        };
        cancel.check()?;
        Audio::new(pcm)
    }
}

fn sentences(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut terminal = false;
    for (i, c) in text.char_indices() {
        if c == '\n' || (c.is_whitespace() && terminal) {
            let part = text[start..i].trim();
            if !part.is_empty() {
                out.push(part);
            }
            start = i + c.len_utf8();
            terminal = false;
        } else if !c.is_whitespace() {
            terminal = matches!(c, '.' | '!' | '?' | '…');
        }
    }
    let tail = text[start..].trim();
    if !tail.is_empty() {
        out.push(tail);
    }
    out
}

fn chunks(phones: &str) -> Result<Vec<String>> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut count = 0;
    for word in phones.split_whitespace() {
        let n = word.chars().count();
        ensure!(n <= MAX_CHUNK_PHONEMES, "one speech word is too long");
        if count + usize::from(!current.is_empty()) + n > MAX_CHUNK_PHONEMES {
            chunks.push(std::mem::take(&mut current));
            count = 0;
        }
        if !current.is_empty() {
            current.push(' ');
            count += 1;
        }
        current.push_str(word);
        count += n;
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    ensure!(!chunks.is_empty(), "no speech phonemes");
    Ok(chunks)
}

pub struct Audio {
    pub pcm: Vec<f32>,
    pub mouth: Mouth,
}
impl Audio {
    pub fn new(pcm: Vec<f32>) -> Result<Self> {
        ensure!(
            !pcm.is_empty() && pcm.len() <= MAX_SAMPLES && pcm.iter().all(|s| s.is_finite()),
            "invalid speech samples"
        );
        Ok(Self {
            mouth: Mouth::from_pcm(&pcm),
            pcm,
        })
    }
}

/// The envelope uses the exact PCM sent to the player; positive offsets delay the beak.
#[derive(Debug, Clone)]
pub struct Mouth {
    samples: usize,
    values: Vec<f64>,
}
impl Mouth {
    fn from_pcm(pcm: &[f32]) -> Self {
        Self {
            samples: pcm.len(),
            values: pcm
                .chunks(480)
                .map(|block| {
                    let rms = (block.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>()
                        / block.len() as f64)
                        .sqrt();
                    ((rms - 0.006) / 0.16).clamp(0.0, 1.0)
                })
                .collect(),
        }
    }
    pub fn duration(&self) -> std::time::Duration {
        std::time::Duration::from_secs_f64(self.samples as f64 / f64::from(SAMPLE_RATE))
    }
    pub fn at(&self, elapsed: std::time::Duration, offset_ms: i32) -> f64 {
        let t = elapsed.as_secs_f64() - f64::from(offset_ms) / 1000.0;
        if t < 0.0 || t >= self.samples as f64 / f64::from(SAMPLE_RATE) {
            return 0.0;
        }
        let position = t * 50.0;
        let i = position.floor() as usize;
        let a = self.values.get(i).copied().unwrap_or(0.0);
        let b = self.values.get(i + 1).copied().unwrap_or(0.0);
        a + (b - a) * position.fract()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    #[test]
    fn bounded_text_rejects_control_bytes_and_non_english_scripts() {
        assert!(validate_text("Hello, human. It costs $3.50.\nI'm ready!").is_ok());
        for text in ["", " \n", "hello\0world", "hello\u{1b}[2J", "日本語"] {
            assert!(validate_text(text).is_err(), "{text:?}");
        }
        assert!(validate_text(&"x".repeat(MAX_TEXT_BYTES)).is_ok());
        assert!(validate_text(&"x".repeat(MAX_TEXT_BYTES + 1)).is_err());
    }
    #[test]
    fn sentence_and_chunk_boundaries_preserve_every_phone() {
        assert_eq!(
            sentences("Hello.  Duck!\nNext? Last… Fine"),
            ["Hello.", "Duck!", "Next?", "Last…", "Fine"]
        );
        assert_eq!(sentences("Pay 3.50 dollars."), ["Pay 3.50 dollars."]);
        let phones = (0..100).map(|_| "həlˈO").collect::<Vec<_>>().join(" ");
        let split = chunks(&phones).unwrap();
        assert!(split.len() > 1);
        assert!(
            split
                .iter()
                .all(|c| c.chars().count() <= MAX_CHUNK_PHONEMES)
        );
        assert_eq!(split.join(" "), phones);
        assert!(chunks(&"x".repeat(MAX_CHUNK_PHONEMES + 1)).is_err());
    }
    #[test]
    fn envelope_uses_pcm_time_and_has_silence_at_the_boundaries() {
        let audio = Audio::new([vec![0.0; 480], vec![0.2; 480], vec![0.0; 480]].concat()).unwrap();
        assert_eq!(audio.mouth.at(Duration::ZERO, 0), 0.0);
        assert!(audio.mouth.at(Duration::from_millis(20), 0) > 0.99);
        assert_eq!(audio.mouth.at(Duration::from_millis(20), 20), 0.0);
        assert!(audio.mouth.at(Duration::ZERO, -20) > 0.99);
        assert_eq!(audio.mouth.at(Duration::from_millis(60), 0), 0.0);
        assert_eq!(audio.mouth.duration(), Duration::from_millis(60));
        assert!(Audio::new(vec![f32::NAN]).is_err());
        assert!(Audio::new(vec![]).is_err());
    }
    #[test]
    fn cancelled_work_never_enters_inference_and_bad_models_fail_before_ort() {
        let cancel = Cancellation::default();
        assert!(cancel.check().is_ok());
        cancel.cancel();
        assert!(cancel.check().is_err());
        let file = tempfile::NamedTempFile::new().unwrap();
        assert!(Engine::new(file.path()).is_err());
    }
}

#[cfg(test)]
mod model_cancellation_tests {
    use super::*;
    #[test]
    #[ignore = "requires the pinned speech model and ONNX Runtime"]
    fn cancellation_interrupts_an_active_onnx_run() {
        let path = std::env::var_os("MICRODUCK_SPEECH_MODEL").expect("set MICRODUCK_SPEECH_MODEL");
        let mut engine = Engine::new(Path::new(&path)).unwrap();
        let cancel = Cancellation::default();
        let job_cancel = cancel.clone();
        let worker = std::thread::spawn(move || {
            engine.synthesize("The duck is speaking a longer sentence to exercise cancellation during neural synthesis rather than at an audio file boundary.",&job_cancel)
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while cancel.run.load().is_none() && !worker.is_finished() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(
            cancel.run.load().is_some(),
            "synthesis ended before cancellation could be tested"
        );
        cancel.cancel();
        assert!(worker.join().unwrap().is_err());
        assert!(cancel.run.load().is_none());
    }
}
