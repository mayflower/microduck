#!/usr/bin/env python3
"""No robot or microphone is used; all commands go to a temporary MuJoCo daemon."""

import argparse
import json
import math
import os
import shutil
import socket
import subprocess
import tempfile
import threading
import time
import wave
from pathlib import Path

parser = argparse.ArgumentParser(
    description="Exercise the real daemon with MuJoCo and replayed microphone audio"
)
root = Path(__file__).resolve().parents[1]
parser.add_argument(
    "--body-python", type=Path, default=root.parent / "microduck_rl/.venv/bin/python"
)
parser.add_argument("--robotd", type=Path, default=root / "target/debug/robotd")
parser.add_argument("--robotctl", type=Path, default=root / "target/debug/robotctl")
parser.add_argument("--policies", type=Path, default=root.parent / "models/policies")
parser.add_argument(
    "--walk-policy",
    default="velstand.onnx",
    help="Walking policy file inside --policies for footwork validation",
)
parser.add_argument(
    "--standing-policy",
    default="none",
    help="Optional standing policy inside --policies",
)
parser.add_argument(
    "--audio",
    "--wav",
    dest="audio",
    type=Path,
    required=True,
    help="WAV, MP3 or another FFmpeg-readable music file (looped)",
)
parser.add_argument("--onnxruntime", default=os.environ.get("ORT_DYLIB_PATH"))
parser.add_argument("--output", type=Path)
parser.add_argument(
    "--check-rhythm",
    action="store_true",
    help="Score physical nods and footfalls against independent kick peaks (requires a steady percussive passage)",
)
parser.add_argument(
    "--demo",
    action="store_true",
    help="Observe the track without injecting artificial silence or manual-control tests",
)
parser.add_argument(
    "--start-seconds",
    type=float,
    default=0.0,
    help="Start both playback and analysis at this position in the source track",
)
parser.add_argument(
    "--acquire-timeout",
    type=float,
    default=90.0,
    help="Allow for musical intros before failing beat acquisition",
)
parser.add_argument(
    "--play-audio",
    action="store_true",
    help="Play the same PCM heard by the simulated mic through the speakers (requires aplay)",
)
parser.add_argument(
    "--observe-seconds",
    type=float,
    default=24.0,
    help="Watch at least several complete dance phrases",
)
parser.add_argument(
    "--viewer", action="store_true", help="Open the MuJoCo viewer on the local desktop"
)
args = parser.parse_args()
if not math.isfinite(args.start_seconds) or args.start_seconds < 0:
    parser.error("--start-seconds must be finite and nonnegative")
if not math.isfinite(args.acquire_timeout) or args.acquire_timeout <= 0:
    parser.error("--acquire-timeout must be finite and positive")
if not math.isfinite(args.observe_seconds) or args.observe_seconds < 12:
    parser.error("--observe-seconds must be at least 12 to cover the dance phrases")
if args.play_audio and shutil.which("aplay") is None:
    parser.error("--play-audio requires aplay on this machine")
if not args.audio.is_file():
    parser.error(f"audio file does not exist: {args.audio}")
if not args.onnxruntime:
    parser.error("set ORT_DYLIB_PATH or --onnxruntime")
tmp = (
    args.output.resolve()
    if args.output
    else Path(tempfile.mkdtemp(prefix="music-dance-sim-"))
)
tmp.mkdir(parents=True, exist_ok=True)
with socket.socket() as free_port:
    free_port.bind(("127.0.0.1", 0))
    port = free_port.getsockname()[1]
print("Artifacts:", tmp, flush=True)
# Decode once. Both the recognizer and speaker player consume these exact samples.
audio = args.audio.resolve()
try:
    with wave.open(str(audio)) as source:
        pcm_ready = (
            source.getparams()[:3] == (1, 2, 16000) and source.getcomptype() == "NONE"
        )
except (wave.Error, EOFError):
    pcm_ready = False
if not pcm_ready or args.start_seconds > 0:
    if shutil.which("ffmpeg") is None:
        parser.error("ffmpeg is required to decode this audio file")
    decoded = tmp / "audio.wav"
    subprocess.run(
        [
            "ffmpeg",
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-ss",
            str(args.start_seconds),
            "-i",
            str(audio),
            "-vn",
            "-ac",
            "1",
            "-ar",
            "16000",
            "-c:a",
            "pcm_s16le",
            str(decoded),
        ],
        check=True,
    )
    audio = decoded
with wave.open(str(audio)) as source:
    if source.getnframes() == 0:
        parser.error("the selected start position leaves no audio")
print(f"Audio starts at {args.start_seconds:.1f}s in the source track", flush=True)
if audio != tmp / "audio.wav":
    shutil.copyfile(audio, tmp / "audio.wav")
    audio = tmp / "audio.wav"
for marker in ["silence", "playback-error"]:
    (tmp / marker).unlink(missing_ok=True)
replay_args = [
    str(root / "scripts/music-dance-replay.py"),
    str(audio),
    "--silence-file",
    str(tmp / "silence"),
    "--clock-file",
    str(tmp / "audio-clock.json"),
]
if args.play_audio:
    replay_args += [
        "--play-audio",
        "--playback-log",
        str(tmp / "playback.log"),
        "--playback-error",
        str(tmp / "playback-error"),
    ]
shim = tmp / "arecord"
shim.write_text(
    "#!/usr/bin/env python3\nimport os,sys\nos.execv(sys.executable,[sys.executable]+"
    + repr(replay_args)
    + ")\n"
)
shim.chmod(0o755)
params = tmp / "robotd.toml"


def toml_path(path):
    return json.dumps(str(path.resolve()))


params.write_text(f"""[policy]
enabled=true
walk={toml_path(args.policies / args.walk_policy)}
stand={json.dumps("none") if args.standing_policy == "none" else toml_path(args.policies / args.standing_policy)}
sitstand="none"
ground_pick="none"
kick_left="none"
kick_right="none"
roulade="none"
[pickup]
enabled=true
model={toml_path(root / "duck-control/models/pickup_detector.onnx")}
[audio]
enabled=true
greet=false
pet_detect=false
bank={toml_path(tmp / "no-sounds")}
[dance]
enabled=false
model={toml_path(root / "pet-detect/models/music_yamnet.onnx")}
[idle]
enabled=false
""")
env = os.environ.copy()
env["PATH"] = str(tmp) + ":" + env["PATH"]
env["ORT_DYLIB_PATH"] = args.onnxruntime
processes = []
logs = []


def spawn(cmd, name):
    f = (tmp / (name + "-" + str(len(processes)) + ".log")).open("w")
    logs.append(f)
    p = subprocess.Popen(cmd, env=env, stdout=f, stderr=subprocess.STDOUT)
    processes.append(p)
    return p


def rpc(method, params=None):
    with socket.socket(socket.AF_UNIX) as s:
        s.settimeout(3)
        s.connect(str(tmp / "robot.sock"))
        f = s.makefile("rwb")
        f.write(
            (
                json.dumps(
                    {
                        "jsonrpc": "2.0",
                        "id": 1,
                        "method": method,
                        "params": params or {},
                    }
                )
                + "\n"
            ).encode()
        )
        f.flush()
        r = json.loads(f.readline())
        assert "error" not in r, r
        return r["result"]


def ctl(action):
    result = subprocess.run(
        [
            str(args.robotctl.absolute()),
            "--robot-socket",
            str(tmp / "robot.sock"),
            "dance",
            action,
        ],
        check=True,
        text=True,
        capture_output=True,
    )
    return json.loads(result.stdout)


def wait(predicate, timeout=30):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        if args.play_audio and (tmp / "playback-error").exists():
            raise RuntimeError((tmp / "playback-error").read_text())
        try:
            x = predicate()
            if x:
                return x
        except (OSError, AssertionError):
            pass
        time.sleep(0.1)
    raise AssertionError("timed out")


try:
    body = spawn(
        [
            str(args.body_python.absolute()),
            "-m",
            "mjlab_microduck.sim.body_server",
            *([] if args.viewer else ["--headless"]),
            "--port",
            str(port),
            "--keyframe",
            "HOME",
        ],
        "body",
    )
    time.sleep(3)
    assert body.poll() is None, (tmp / "body-0.log").read_text()

    def daemon():
        return spawn(
            [
                str(args.robotd.absolute()),
                "--sim",
                f"127.0.0.1:{port}",
                "--socket",
                str(tmp / "robot.sock"),
                "--params",
                str(params),
            ],
            "robotd",
        )

    robot = daemon()
    wait(lambda: rpc("robot.dance"))
    print("started", rpc("robot.dance"), flush=True)
    assert ctl("on")["enabled"]
    assert rpc("robot.enable", {"on": True})["accepted"]
    print("enabled", flush=True)
    last_wait_state = None

    def acquiring():
        global last_wait_state
        status = rpc("robot.dance")
        current = (status["state"], status["reason"])
        if current != last_wait_state:
            print("Dance status:", json.dumps(status), flush=True)
            last_wait_state = current
        return status if status["state"] == "dancing" else None

    status = wait(acquiring, timeout=args.acquire_timeout)
    print("dancing", status, flush=True)
    (tmp / "dance-pattern.json").write_text(json.dumps({"steps_per_beat": 1}))
    shutil.copyfile(tmp / "audio-clock.json", tmp / "observation-audio-clock.json")
    # Also read the simulator's ground truth: an exaggerated command that the
    # policy ignores must not pass as a more expressive physical dance.
    with (
        socket.socket(socket.AF_UNIX) as s,
        socket.create_connection(("127.0.0.1", port), timeout=3) as physics,
    ):
        physics.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        body_io = physics.makefile("rwb")
        body_io.write(b'{"op":"hello","protocol":1,"joints":15}\n')
        body_io.flush()
        assert json.loads(body_io.readline())["protocol"] == 1
        s.settimeout(3)
        s.connect(str(tmp / "robot.sock"))
        f = s.makefile("rwb")
        f.write(
            b'{"jsonrpc":"2.0","id":2,"method":"robot.subscribe","params":{"hz":20}}\n'
        )
        f.flush()
        if args.play_audio:
            assert not (tmp / "playback-error").exists(), "speaker playback failed"
            print(
                "Speaker playback uses the same paced PCM stream as music analysis",
                flush=True,
            )
        end = time.monotonic() + args.observe_seconds
        states = []
        bodies = []
        stopped = threading.Event()
        sampling_errors = []

        def sample_physics():
            try:
                next_sample = time.monotonic()
                while not stopped.is_set():
                    body_io.write(b'{"op":"read"}\n')
                    body_io.flush()
                    sample = json.loads(body_io.readline())
                    sample["t_ns"] = time.monotonic_ns()
                    bodies.append(sample)
                    next_sample += 0.01
                    stopped.wait(max(0, next_sample - time.monotonic()))
            except (OSError, ValueError, EOFError) as error:
                sampling_errors.append(str(error))

        sampler = threading.Thread(target=sample_physics)
        sampler.start()
        try:
            while time.monotonic() < end:
                r = json.loads(f.readline())
                if r.get("method") == "robot.state":
                    states.append(r["params"])
        finally:
            stopped.set()
            sampler.join(4)
        assert not sampler.is_alive(), "physics sampler did not stop"
        assert not sampling_errors, sampling_errors
        (tmp / "body-states.json").write_text(json.dumps(bodies))
        (tmp / "states.json").write_text(json.dumps(states))
        assert states, "no states"
        assert all(not s["safety"]["fallen"] for s in states), "fell while dancing"
        assert all(
            math.hypot(*s["move"]["applied"][:2]) <= 0.38 + 1e-8
            and abs(s["move"]["applied"][2]) <= 0.8 + 1e-8
            for s in states
        ), "dance velocity exceeds its envelope"
        for i, limit in enumerate([0, math.radians(12), math.radians(16), 0]):
            assert max(abs(s["head"][i]) for s in states) <= limit + 1e-8, (
                "head exceeds dance bounds"
            )
        assert all(s["policy"] in ("stand", "walk") for s in states), {
            s["policy"] for s in states
        }

        def span(values):
            return max(values) - min(values)

        settled = bodies[min(40, len(bodies) // 4) :]
        report = {
            "duration_s": args.observe_seconds,
            "frames": len(states),
            "body_roll_span_deg": span(
                [
                    math.degrees(
                        math.atan2(-s["imu"]["gravity"][1], -s["imu"]["gravity"][2])
                    )
                    for s in settled
                ]
            ),
            "body_pitch_span_deg": span(
                [
                    math.degrees(math.asin(max(-1, min(1, s["imu"]["gravity"][0]))))
                    for s in settled
                ]
            ),
            "head_pitch_span_deg": span(
                [math.degrees(s["positions"][6]) for s in settled]
            ),
            "head_yaw_span_deg": span(
                [math.degrees(s["positions"][7]) for s in settled]
            ),
            "trunk_height_span_mm": 1000 * span([s["trunk_z"] for s in settled]),
            "minimum_trunk_height_m": min(s["trunk_z"] for s in settled),
            "max_tilt_deg": max(
                math.degrees(math.acos(max(-1, min(1, -s["imu"]["gravity"][2]))))
                for s in bodies
            ),
            "fallen_frames": sum(s["safety"]["fallen"] for s in states),
            "picked_up_frames": sum(s["safety"]["picked_up"] for s in states),
        }
        (tmp / "motion.json").write_text(json.dumps(report, indent=2) + "\n")
        print("Measured motion:", json.dumps(report), flush=True)
        report["trunk_x_span_m"] = span([s["trunk"][0] for s in settled])
        report["trunk_y_span_m"] = span([s["trunk"][1] for s in settled])
        report["max_radius_m"] = max(
            math.hypot(
                s["trunk"][0] - bodies[0]["trunk"][0],
                s["trunk"][1] - bodies[0]["trunk"][1],
            )
            for s in bodies
        )
        (tmp / "motion.json").write_text(json.dumps(report, indent=2) + "\n")
        print("Path:", json.dumps(report), flush=True)
        assert report["trunk_x_span_m"] >= 0.04 and report["trunk_y_span_m"] >= 0.04, (
            "footwork must move on both axes"
        )
        assert report["max_radius_m"] < 0.55, "dance escaped its local area"
        assert report["head_yaw_span_deg"] >= 20, "head sweep is too small"
        assert report["head_pitch_span_deg"] >= 6, "head nod is too small"
    print("balanced", len(states), "frames", flush=True)
    if args.check_rhythm:
        subprocess.run(
            [
                str(args.body_python.absolute()),
                str(root / "scripts/score-music-dance-timing.py"),
                str(tmp),
                "--check",
            ],
            check=True,
        )
    subprocess.run(
        [
            str(args.body_python.absolute()),
            str(root / "scripts/score-music-dance-footwork.py"),
            str(tmp),
            "--check",
            *(["--check-rhythm"] if args.check_rhythm else []),
        ],
        check=True,
    )
    if not args.demo:
        (tmp / "silence").touch()
        wait(lambda: rpc("robot.dance")["reason"] == "no_music_or_beat", 8)
        wait(lambda: rpc("robot.dance")["state"] == "waiting", 3)
        print("music loss fades to neutral", flush=True)
        (tmp / "silence").unlink()
        last_wait_state = None
        wait(acquiring, args.acquire_timeout)
        assert rpc(
            "robot.head",
            {"neck_pitch": 0, "head_pitch": 0.02, "head_yaw": 0, "head_roll": 0},
        )["accepted"]
        wait(lambda: rpc("robot.dance")["reason"] == "manual_control", 2)
        print("manual pause", flush=True)
        last_wait_state = None
        wait(acquiring, args.acquire_timeout)
        print("resumed", flush=True)
        assert not ctl("off")["enabled"]
        assert not ctl("status")["enabled"]
        print("disabled", flush=True)
        assert rpc("robot.dance.set", {"enabled": True})["accepted"]
        robot.terminate()
        robot.wait(5)
        robot = daemon()
        wait(lambda: rpc("robot.dance")["enabled"])
        assert rpc("robot.dance")["state"] != "dancing"
        print("persistent across restart; motors remain disabled", flush=True)
finally:
    for p in reversed(processes):
        if p.poll() is None:
            p.terminate()
    for p in reversed(processes):
        try:
            p.wait(5)
        except subprocess.TimeoutExpired:
            p.kill()
            p.wait()
    for f in logs:
        f.close()
