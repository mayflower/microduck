//! The approved B processor is a standalone native tool, supervised by the synthesis worker.
use crate::{Cancellation, MAX_SAMPLES};
use anyhow::{Context, Result, bail, ensure};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

const VERSION: &str = "duck-voice-color 1 (Rubber Band 4.0.0; preset B)";
pub(crate) struct Color {
    command: PathBuf,
}
impl Color {
    pub fn discover() -> Result<Self> {
        let path = if let Some(path) = std::env::var_os("MICRODUCK_VOICE_COLOR") {
            PathBuf::from(path)
        } else {
            let exe = std::env::current_exe()?;
            let parent = exe.parent().context("executable has no parent")?;
            let parent = if parent
                .file_name()
                .is_some_and(|n| n == "deps" || n == "examples")
            {
                parent
                    .parent()
                    .context("Cargo executable has no target directory")?
            } else {
                parent
            };
            parent.join("duck-voice-color")
        };
        Self::new(path)
    }
    fn new(command: PathBuf) -> Result<Self> {
        let version = exchange(&command, &["--version"], Vec::new(), 128,
            Duration::from_secs(2), &Cancellation::default())
            .with_context(|| format!("checking nasal voice processor {}; install the matching full daemon bundle or select clean speech",command.display()))?;
        ensure!(
            std::str::from_utf8(&version)?.trim() == VERSION,
            "wrong voice processor version"
        );
        Ok(Self { command })
    }
    pub fn apply(&self, pcm: Vec<f32>, cancel: &Cancellation) -> Result<Vec<f32>> {
        cancel.check()?;
        ensure!(pcm.len() <= MAX_SAMPLES, "speech buffer exceeds limit");
        let size = pcm.len() * 4;
        let input = pcm.into_iter().flat_map(f32::to_le_bytes).collect();
        let output = exchange(
            &self.command,
            &[
                "--sample-rate",
                "24000",
                "--formant",
                "1.5",
                "--loudness",
                "-21",
            ],
            input,
            size,
            Duration::from_secs(20),
            cancel,
        )?;
        ensure!(output.len() == size, "voice processor changed sample count");
        let samples: Vec<f32> = output
            .as_chunks::<4>()
            .0
            .iter()
            .map(|s| f32::from_le_bytes(*s))
            .collect();
        ensure!(
            samples.iter().all(|s| s.is_finite() && s.abs() <= 0.8001),
            "voice processor returned invalid PCM"
        );
        cancel.check()?;
        Ok(samples)
    }
}

struct OwnedChild(Option<Child>);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take()
            && !matches!(child.try_wait(), Ok(Some(_)))
        {
            let _ = child.kill();
            // Cancellation returns promptly even if the OS has not reaped it yet.
            let _ = std::thread::Builder::new()
                .name("voice-color-reap".into())
                .spawn(move || {
                    let _ = child.wait();
                });
        }
    }
}

fn exchange(
    command: &Path,
    args: &[&str],
    input: Vec<u8>,
    max_output: usize,
    timeout: Duration,
    cancel: &Cancellation,
) -> Result<Vec<u8>> {
    cancel.check()?;
    let child = Command::new(command)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("starting {}", command.display()))?;
    let mut owned = OwnedChild(Some(child));
    let child = owned.0.as_mut().expect("owned child");
    let mut stdin = child.stdin.take().context("voice processor has no stdin")?;
    let stdout = child
        .stdout
        .take()
        .context("voice processor has no stdout")?;
    let (write_tx, write_rx) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("voice-color-in".into())
        .spawn(move || {
            // robotd restores SIGPIPE's default disposition. A canceled processor must produce
            // EPIPE here, never terminate the daemon that owns the motor loop.
            #[cfg(unix)]
            unsafe {
                let mut set: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&mut set);
                libc::sigaddset(&mut set, libc::SIGPIPE);
                libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
            }
            let result = stdin.write_all(&input);
            drop(stdin);
            let _ = write_tx.send(result);
        })?;
    let (read_tx, read_rx) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("voice-color-out".into())
        .spawn(move || {
            let mut output = Vec::new();
            let result = stdout.take(max_output as u64 + 1).read_to_end(&mut output);
            let _ = read_tx.send(result.map(|_| output));
        })?;
    let deadline = Instant::now() + timeout;
    let mut output = None;
    let mut written = false;
    loop {
        cancel.check()?;
        ensure!(Instant::now() < deadline, "voice processor timed out");
        if !written && let Ok(result) = write_rx.try_recv() {
            result.context("writing voice PCM")?;
            written = true;
        }
        if output.is_none()
            && let Ok(result) = read_rx.try_recv()
        {
            let data = result.context("reading colored PCM")?;
            ensure!(
                data.len() <= max_output,
                "voice processor output exceeds limit"
            );
            output = Some(data);
        }
        match owned.0.as_mut().expect("owned child").try_wait()? {
            Some(status) if !status.success() => bail!("voice processor failed ({status})"),
            Some(_) if written && output.is_some() => return Ok(output.expect("received output")),
            _ => {}
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    fn script(body: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("processor");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        (dir, path)
    }
    #[test]
    fn incompatible_missing_failed_and_oversized_processors_are_errors() {
        assert!(Color::new(PathBuf::from("/missing/voice-color")).is_err());
        let (_dir, path) = script("printf 'wrong processor\\n'");
        assert!(Color::new(path).is_err());
        for body in ["exit 7", "printf 'too many bytes'"] {
            let (_dir, path) = script(body);
            assert!(
                exchange(
                    &path,
                    &[],
                    Vec::new(),
                    4,
                    Duration::from_secs(1),
                    &Default::default()
                )
                .is_err()
            );
        }
    }
    #[test]
    fn processor_changes_in_length_or_nonfinite_pcm_are_refused() {
        let (_dir, path) = script("cat >/dev/null; printf '\\000\\000\\300\\177'");
        let processor = Color { command: path };
        assert!(processor.apply(vec![0.1], &Default::default()).is_err());
        let (_dir, path) = script("cat >/dev/null; printf 'x'");
        let processor = Color { command: path };
        assert!(processor.apply(vec![0.1], &Default::default()).is_err());
    }
    #[test]
    fn cancellation_and_timeout_stop_the_native_processor() {
        let (_dir, path) = script("exec sleep 30");
        let start = Instant::now();
        assert!(
            exchange(
                &path,
                &[],
                Vec::new(),
                16,
                Duration::from_millis(30),
                &Default::default()
            )
            .is_err()
        );
        assert!(start.elapsed() < Duration::from_secs(1));
        let cancel = Cancellation::default();
        let other = cancel.clone();
        let task = std::thread::spawn(move || {
            exchange(&path, &[], Vec::new(), 16, Duration::from_secs(5), &other)
        });
        std::thread::sleep(Duration::from_millis(30));
        cancel.cancel();
        assert!(task.join().unwrap().is_err());
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn a_dead_processor_pipe_cannot_deliver_sigpipe_to_robotd() {
        const MARKER: &str = "MICRODUCK_COLOR_SIGPIPE_TEST";
        if std::env::var_os(MARKER).is_none() {
            let status = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "color::tests::a_dead_processor_pipe_cannot_deliver_sigpipe_to_robotd",
                ])
                .env(MARKER, "1")
                .status()
                .unwrap();
            assert!(status.success(), "voice writer terminated its daemon");
            return;
        }
        unsafe {
            libc::signal(libc::SIGPIPE, libc::SIG_DFL);
        }
        let (_dir, path) = script("exit 7");
        assert!(
            exchange(
                &path,
                &[],
                vec![0; MAX_SAMPLES * 4],
                4,
                Duration::from_secs(1),
                &Default::default()
            )
            .is_err()
        );
    }
}
