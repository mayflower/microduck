//! Real daemon, native local TTS and paced PCM playback. Requires opt-in local model assets.
#![cfg(unix)]
use duck_ipc_proto::{
    Call, EnableParams, Id, IntentResult, MouthParams, RobotState, SayParams, SubscribeParams,
};
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::{fs::PermissionsExt, net::UnixStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Daemon(Child);
impl Drop for Daemon {
    fn drop(&mut self) {
        unsafe {
            libc::kill(self.0.id() as i32, libc::SIGTERM);
        }
        let end = Instant::now() + Duration::from_secs(3);
        while matches!(self.0.try_wait(), Ok(None)) && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn connect(path: &Path) -> BufReader<UnixStream> {
    let stream = UnixStream::connect(path).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    BufReader::new(stream)
}
fn send(reader: &mut BufReader<UnixStream>, call: Call) {
    let request = duck_ipc_proto::Request::call(Id::Number(1), &call);
    writeln!(
        reader.get_mut(),
        "{}",
        serde_json::to_string(&request).unwrap()
    )
    .unwrap();
}
fn response(reader: &mut BufReader<UnixStream>) -> serde_json::Value {
    let mut line = String::new();
    assert!(reader.read_line(&mut line).unwrap() > 0);
    serde_json::from_str(&line).unwrap()
}
fn rpc(path: &Path, call: Call) -> IntentResult {
    let mut reader = connect(path);
    send(&mut reader, call);
    let value = response(&mut reader);
    serde_json::from_value(value["result"].clone()).unwrap()
}
fn state(reader: &mut BufReader<UnixStream>) -> RobotState {
    loop {
        let value = response(reader);
        if value["method"] == "robot.state" {
            return serde_json::from_value(value["params"].clone()).unwrap();
        }
    }
}
fn flaps(path: &Path) -> f64 {
    let mut reader = connect(path);
    send(&mut reader, Call::RobotVetbook);
    response(&mut reader)["result"]["life"]["tallies"]["beak_flaps"]
        .as_f64()
        .unwrap_or(0.0)
}
fn say(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let result = rpc(
            path,
            Call::RobotSay(SayParams {
                text: "Hello, human. Duck reporting for duty.".into(),
            }),
        );
        if result.accepted {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "speech never became available: {:?}",
            result.reason
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}
fn wait_spoken(reader: &mut BufReader<UnixStream>, log_path: &Path, count: usize) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        state(reader);
        let log = std::fs::read_to_string(log_path).unwrap();
        if log.matches("SpeechSpoken").count() >= count {
            return;
        }
        assert!(Instant::now() < deadline, "speech never completed: {log}");
    }
}

struct Fixture {
    // Stop the daemon before removing its files.
    daemon: Daemon,
    _dir: tempfile::TempDir,
    socket: std::path::PathBuf,
    log: std::path::PathBuf,
}
impl Fixture {
    fn start(behaviors: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let model = std::env::var_os("MICRODUCK_SPEECH_MODEL").expect("set MICRODUCK_SPEECH_MODEL");
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let player = bin.join("aplay");
        // Consume actual PCM; the production writer supplies its real-time pacing.
        std::fs::write(&player, "#!/bin/sh\nexec /bin/cat >/dev/null\n").unwrap();
        std::fs::set_permissions(&player, std::fs::Permissions::from_mode(0o700)).unwrap();
        let params = dir.path().join("robotd.toml");
        let config = format!(
            r#"[audio]
enabled = true
greet = {behaviors}
pet_detect = false
bank = {bank:?}
[speech]
enabled = true
behaviors = {behaviors}
model = {model:?}
[pickup]
enabled = false
"#,
            bank = dir.path().join("missing-bank"),
            model = Path::new(&model)
        );
        std::fs::write(&params, config).unwrap();
        let socket = dir.path().join("robotd.sock");
        let log = dir.path().join("robotd.log");
        let daemon = Daemon(
            Command::new(env!("CARGO_BIN_EXE_robotd"))
                .args(["--fake", "--no-policy", "--socket"])
                .arg(&socket)
                .arg("--params")
                .arg(params)
                .arg("--vetbook")
                .arg(dir.path().join("vetbook.json"))
                .env("PATH", bin)
                .env("RUST_LOG", "info")
                .stdout(Stdio::null())
                .stderr(File::create(&log).unwrap())
                .spawn()
                .unwrap(),
        );
        let mut fixture = Self {
            daemon,
            _dir: dir,
            socket,
            log,
        };
        let deadline = Instant::now() + Duration::from_secs(15);
        while !fixture.socket.exists() {
            assert!(fixture.daemon.0.try_wait().unwrap().is_none());
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        fixture
    }
    fn subscribe(&self) -> BufReader<UnixStream> {
        let mut reader = connect(&self.socket);
        send(
            &mut reader,
            Call::RobotSubscribe(SubscribeParams { hz: Some(50) }),
        );
        assert_eq!(response(&mut reader)["result"]["accepted"], true);
        reader
    }
}

#[test]
#[ignore = "requires the pinned speech model and ONNX Runtime; no hardware"]
fn real_native_speech_respects_disabled_servos_and_a_persistent_manual_grip() {
    let fixture = Fixture::start(false);
    let socket = &fixture.socket;
    let log_path = &fixture.log;
    let mut reader = fixture.subscribe();
    state(&mut reader);
    say(socket);
    wait_spoken(&mut reader, log_path, 1);
    assert_eq!(
        flaps(socket),
        0.0,
        "disabled speech animated or tallied the beak"
    );
    assert!(rpc(socket, Call::RobotInit).accepted);
    assert!(
        rpc(
            socket,
            Call::RobotEnable(EnableParams {
                on: true,
                toggle: false
            })
        )
        .accepted
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut homing = false;
    loop {
        let frame = state(&mut reader);
        homing |= frame.policy == "homing";
        if homing && frame.policy != "homing" {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "homing did not finish: {}",
            frame.policy
        );
    }
    say(socket);
    let mut mouth = Vec::new();
    for _ in 0..100 {
        assert!(rpc(socket, Call::RobotMouth(MouthParams { open: 0.0 })).accepted);
        mouth.push(state(&mut reader).targets[duck_control::model::MOUTH_INDEX]);
    }
    let range = mouth.iter().copied().fold(f64::NEG_INFINITY, f64::max)
        - mouth.iter().copied().fold(f64::INFINITY, f64::min);
    assert!(range > 0.1, "neutral keepalives stole speech: {mouth:?}");
    wait_spoken(&mut reader, log_path, 2);
    assert!(
        flaps(socket) > 0.0,
        "speech did not reach the effective beak tally"
    );
    say(socket);
    for _ in 0..35 {
        state(&mut reader);
    }
    assert!(rpc(socket, Call::RobotMouth(MouthParams { open: 0.8 })).accepted);
    let expected = duck_control::model::mouth_target(0.8);
    let mut observed = false;
    for _ in 0..190 {
        let mouth = state(&mut reader).targets[duck_control::model::MOUTH_INDEX];
        if (mouth - expected).abs() < 1e-9 {
            observed = true;
        }
        if observed {
            assert!(
                (mouth - expected).abs() < 1e-9,
                "speech completion overwrote the grip"
            );
        }
    }
    assert!(observed, "manual grip never won");
    assert_eq!(
        std::fs::read_to_string(log_path)
            .unwrap()
            .matches("SpeechSpoken")
            .count(),
        3
    );
}

#[test]
#[ignore = "requires the pinned speech model and ONNX Runtime; no hardware"]
fn startup_behavior_speaks_once_without_any_say_request() {
    let fixture = Fixture::start(true);
    let socket = &fixture.socket;
    let log_path = &fixture.log;
    let mut reader = fixture.subscribe();
    wait_spoken(&mut reader, log_path, 1);
    // Observe past both the startup grace period and the global cooldown: hello is one-shot.
    let until = Instant::now() + Duration::from_secs(16);
    while Instant::now() < until {
        state(&mut reader);
    }
    let log = std::fs::read_to_string(log_path).unwrap();
    assert_eq!(log.matches("behavior speech accepted").count(), 1, "{log}");
    assert!(log.contains("greeting"), "{log}");
    assert_eq!(log.matches("SpeechSpoken").count(), 1, "{log}");
    assert_eq!(
        flaps(socket),
        0.0,
        "startup greeting animated disabled servos"
    );
}
