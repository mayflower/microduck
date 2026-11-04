//! These opt-in checks use the pinned local model. Default workspace tests never download it.
use std::{path::PathBuf, time::Duration};
fn model() -> PathBuf {
    std::env::var_os("MICRODUCK_SPEECH_MODEL")
        .expect("set MICRODUCK_SPEECH_MODEL")
        .into()
}
#[test]
#[ignore = "requires the separately installed pinned Paradee model and ONNX Runtime"]
fn native_text_synthesizes_heart_without_external_helpers() {
    let mut engine = duck_speech::Engine::new(&model()).unwrap();
    let phones = engine.phonemes("Hello world.").unwrap();
    assert_eq!(phones, "həlˈO wˈɜɹld.");
    let audio = engine
        .synthesize(
            "Hello, human. Duck reporting for duty.",
            &Default::default(),
        )
        .unwrap();
    assert_eq!(
        audio.pcm.len(),
        86_400,
        "approved sample's phoneme durations changed"
    );
    assert!(audio.pcm.iter().any(|v| v.abs() > 0.1));
    assert!(audio.mouth.at(Duration::from_millis(500), 0) > 0.0);
    let fresh = engine
        .synthesize(
            "This is newly requested speech. My battery is at 73 percent.",
            &Default::default(),
        )
        .unwrap();
    assert_ne!(audio.pcm.len(), fresh.pcm.len());
    assert!(fresh.pcm.iter().all(|v| v.is_finite()));
    let cancelled = duck_speech::Cancellation::default();
    cancelled.cancel();
    assert!(engine.synthesize("Never spoken.", &cancelled).is_err());
}
