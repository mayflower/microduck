//! Optional regression against the exact listening pair approved by the user.
use std::{
    io::Write,
    process::{Command, Stdio},
};
fn wav(path: &std::ffi::OsStr) -> Vec<f32> {
    let mut reader = hound::WavReader::open(path).unwrap();
    assert_eq!(reader.spec().sample_rate, 24_000);
    assert_eq!(reader.spec().channels, 1);
    reader
        .samples::<i16>()
        .map(|s| f32::from(s.unwrap()) / 32768.0)
        .collect()
}
#[test]
#[ignore = "requires MICRODUCK_B_SOURCE and MICRODUCK_B_REFERENCE audition WAVs"]
fn native_processor_matches_the_approved_b_reference() {
    let source = wav(&std::env::var_os("MICRODUCK_B_SOURCE").expect("set MICRODUCK_B_SOURCE"));
    let reference =
        wav(&std::env::var_os("MICRODUCK_B_REFERENCE").expect("set MICRODUCK_B_REFERENCE"));
    let mut child = Command::new(env!("CARGO_BIN_EXE_duck-voice-color"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let bytes: Vec<_> = source.iter().flat_map(|s| s.to_le_bytes()).collect();
    child.stdin.take().unwrap().write_all(&bytes).unwrap();
    let result = child.wait_with_output().unwrap();
    assert!(result.status.success());
    let native: Vec<f64> = result
        .stdout
        .as_chunks::<4>()
        .0
        .iter()
        .map(|s| f64::from(f32::from_le_bytes(*s)))
        .collect();
    assert_eq!(native.len(), reference.len());
    let xy: f64 = native
        .iter()
        .zip(&reference)
        .map(|(a, b)| a * f64::from(*b))
        .sum();
    let xx: f64 = native.iter().map(|s| s * s).sum();
    let yy: f64 = reference.iter().map(|s| f64::from(*s).powi(2)).sum();
    let correlation = xy / (xx * yy).sqrt();
    let gain = xy / xx;
    assert!(
        correlation > 0.9999,
        "color changed: correlation {correlation}"
    );
    assert!(
        (20.0 * gain.log10()).abs() < 1.0,
        "level differs by more than 1 dB"
    );
}
