#![cfg(unix)]

use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command};

fn installer(path: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_sounds"));
    cmd.args(["speech", "install"]).env("PATH", path);
    cmd
}

#[test]
fn disabled_speech_never_needs_a_downloader_or_creates_an_asset() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("robotd.toml");
    let model = dir.path().join("speech.onnx");
    fs::write(
        &config,
        format!("[speech]\nenabled = false\nmodel = {:?}\n", model),
    )
    .unwrap();
    let result = installer(dir.path())
        .arg("--if-enabled")
        .arg(config)
        .output()
        .unwrap();
    assert!(result.status.success(), "{:?}", result);
    assert!(!model.exists());
    assert!(!model.with_extension("onnx.lock").exists());
}

#[test]
fn untrusted_downloads_cannot_replace_the_installed_file() {
    let dir = tempfile::tempdir().unwrap();
    let model = dir.path().join("speech.onnx");
    fs::write(
        &model,
        b"existing asset, preserved until a verified replacement",
    )
    .unwrap();
    // A successful HTTP response is not sufficient: its content must match the pinned digest.
    let downloader = dir.path().join("curl");
    fs::write(&downloader, "#!/bin/sh\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = --output ]; then shift; printf 'untrusted response' > \"$1\"; exit 0; fi\n  shift\ndone\nexit 1\n").unwrap();
    fs::set_permissions(&downloader, fs::Permissions::from_mode(0o755)).unwrap();
    let result = installer(dir.path())
        .arg("--model")
        .arg(&model)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("unexpected speech model size"));
    assert_eq!(
        fs::read(&model).unwrap(),
        b"existing asset, preserved until a verified replacement"
    );
    assert_eq!(
        fs::read_dir(dir.path()).unwrap().count(),
        3,
        "no staged download remains"
    );
}

#[test]
fn concurrent_installation_is_refused_before_download() {
    let dir = tempfile::tempdir().unwrap();
    let model = dir.path().join("speech.onnx");
    let lock = fs::File::create(model.with_extension("onnx.lock")).unwrap();
    lock.try_lock().unwrap();
    let result = installer(dir.path())
        .arg("--model")
        .arg(&model)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("another speech installer is running")
    );
    assert!(!model.exists());
}

#[test]
fn a_restrictive_installer_umask_cannot_hide_new_model_directories_from_robotd() {
    let dir = tempfile::tempdir().unwrap();
    let original_mode = fs::metadata(dir.path()).unwrap().permissions().mode();
    let model = dir.path().join("models/nested/speech.onnx");
    let result = Command::new("/bin/sh")
        .args(["-c", "umask 077; exec \"$@\"", "installer"])
        .arg(env!("CARGO_BIN_EXE_sounds"))
        .args(["speech", "install", "--model"])
        .arg(&model)
        .env("PATH", dir.path())
        .output()
        .unwrap();
    // No downloader is provided: directory setup must be safe even before the download starts.
    assert!(!result.status.success());
    for path in [dir.path().join("models"), dir.path().join("models/nested")] {
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }
    assert_eq!(
        fs::metadata(dir.path()).unwrap().permissions().mode(),
        original_mode
    );
    assert!(!model.exists());
}
