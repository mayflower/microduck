#!/usr/bin/env python3
"""Render actual saved MuJoCo poses and clock-aligned music to an MP4, at real speed.

Run with the demo's Python environment and simulator PYTHONPATH. Set MUJOCO_GL=egl
for offscreen rendering on Linux. FFmpeg supplies video encoding and the title overlay.
"""

import argparse
import hashlib
import json
import math
import subprocess
from pathlib import Path

import mujoco
import numpy as np
from mjlab_microduck.sim.body_server import DEFAULT_SCENE, Body, World

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("directory", type=Path, help="Output from test-music-dance-sim.py")
parser.add_argument(
    "--audio", type=Path, help="Original track; defaults to captured audio.wav"
)
parser.add_argument(
    "--source-start-seconds", type=float, default=0, help="Original track cue"
)
parser.add_argument(
    "--start-seconds", type=float, default=2, help="Skip initial saved poses"
)
parser.add_argument("--duration", type=float, default=30)
parser.add_argument(
    "--azimuth", type=float, default=225, help="Fixed camera view in degrees"
)
parser.add_argument("--output", type=Path, required=True)
args = parser.parse_args()
if (
    any(
        not math.isfinite(v) or v < 0
        for v in [args.source_start_seconds, args.start_seconds, args.duration]
    )
    or args.duration <= 0
):
    parser.error(
        "cue/start must be nonnegative and duration must be positive and finite"
    )
if args.audio is None and args.source_start_seconds:
    parser.error("--source-start-seconds requires the original --audio track")

poses_file = args.directory / "body-states.json"
frames = json.loads(poses_file.read_text())
clock = json.loads((args.directory / "observation-audio-clock.json").read_text())
if len(frames) < 2:
    parser.error("need at least two recorded poses")
origin = frames[0]["t_ns"]
times = np.asarray([(f["t_ns"] - origin) / 1e9 for f in frames])
if not np.all(np.diff(times) > 0):
    parser.error("recorded pose timestamps must increase strictly")
if args.start_seconds + args.duration > times[-1]:
    parser.error("record more poses: the requested clip exceeds the recorded interval")

audio = args.audio or args.directory / "audio.wav"
audio_duration = float(
    subprocess.check_output(
        [
            "ffprobe",
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            str(audio),
        ],
        text=True,
    )
)
latency = clock["estimated_playback_latency_ms"] / 1000
elapsed = (
    (origin - clock["source_start_monotonic_ns"]) / 1e9 + args.start_seconds - latency
)
loop_duration = audio_duration - args.source_start_seconds
if loop_duration <= 0 or elapsed < 0:
    parser.error("audio cue/clock leaves no usable soundtrack")
seek = args.source_start_seconds + elapsed % loop_duration
if seek + args.duration > audio_duration:
    parser.error(
        "clip crosses the audio replay loop; choose an earlier or shorter interval"
    )

positions = np.asarray([f["positions"] for f in frames])
trunks = np.asarray([f["trunk"] for f in frames])
quaternions = np.asarray([f["imu"]["quat"] for f in frames])
for i in range(1, len(quaternions)):
    if np.dot(quaternions[i - 1], quaternions[i]) < 0:
        quaternions[i] *= -1

world = World(DEFAULT_SCENE, 1)
body = Body(world, 0)
model, data = world.model, world.data
width, height, fps = 1280, 720, 30
model.vis.global_.offwidth = width
model.vis.global_.offheight = height
camera = mujoco.MjvCamera()
selected = (times >= args.start_seconds) & (times <= args.start_seconds + args.duration)
camera.lookat[:] = [*trunks[selected, :2].mean(axis=0), 0.16]
camera.distance, camera.azimuth, camera.elevation = 0.72, args.azimuth, -17
args.output.parent.mkdir(parents=True, exist_ok=True)
overlay = (
    "drawtext=text='Microduck dances to music':x=36:y=30:fontsize=32:fontcolor=white:shadowcolor=black:shadowx=1:shadowy=1,"
    "drawtext=text='MuJoCo simulation | real-time motion':x=38:y=74:fontsize=20:fontcolor=white:shadowcolor=black:shadowx=1:shadowy=1"
)
command = [
    "ffmpeg",
    "-hide_banner",
    "-loglevel",
    "error",
    "-y",
    "-f",
    "rawvideo",
    "-pixel_format",
    "rgb24",
    "-video_size",
    f"{width}x{height}",
    "-framerate",
    str(fps),
    "-i",
    "pipe:0",
    "-ss",
    str(seek),
    "-i",
    str(audio),
    "-map",
    "0:v:0",
    "-map",
    "1:a:0",
    "-t",
    str(args.duration),
    "-vf",
    overlay,
    "-c:v",
    "libx264",
    "-preset",
    "medium",
    "-crf",
    "20",
    "-pix_fmt",
    "yuv420p",
    "-c:a",
    "aac",
    "-b:a",
    "192k",
    "-ar",
    "48000",
    "-movflags",
    "+faststart",
    str(args.output),
]
frame_count = round(args.duration * fps)
if abs(frame_count / fps - args.duration) > 1e-8:
    parser.error("duration must be a whole number of 30 fps frames")
sample_times = args.start_seconds + np.arange(frame_count) / fps


def resample(values):
    return np.column_stack(
        [np.interp(sample_times, times, column) for column in values.T]
    )


trunk_samples = resample(trunks)
pose_samples = resample(positions)
quaternion_samples = resample(quaternions)
quaternion_samples /= np.linalg.norm(quaternion_samples, axis=1, keepdims=True)
# A music break can stop the duck after acquisition. Reject such a take rather
# than turning a recording of a standing duck into a "dancing" preview.
for first in range(0, frame_count, fps):
    section = pose_samples[first : first + 2 * fps]
    if len(section) < fps:
        continue
    if np.ptp(section[:, 6]) < math.radians(6) or any(
        np.ptp(section[:, knee]) < math.radians(5) for knee in [3, 13]
    ):
        parser.error(
            f"stationary interval near {first / fps:.0f}s; choose a continuous dance passage"
        )
encoder = subprocess.Popen(command, stdin=subprocess.PIPE)
try:
    with mujoco.Renderer(model, height, width) as renderer:
        for index in range(frame_count):
            data.qpos[body.trunk : body.trunk + 3] = trunk_samples[index]
            data.qpos[body.trunk + 3 : body.trunk + 7] = quaternion_samples[index]
            data.qpos[body.qpos_adr] = pose_samples[index][body.to_wire]
            mujoco.mj_forward(model, data)
            renderer.update_scene(data, camera=camera)
            encoder.stdin.write(renderer.render().tobytes())
            if index % 150 == 0:
                print(f"Rendered {index}/{frame_count} frames", flush=True)
    encoder.stdin.close()
    if encoder.wait() != 0:
        raise RuntimeError("FFmpeg encoding failed")
finally:
    if encoder.poll() is None:
        encoder.terminate()
        encoder.wait()

metadata = {
    "duration_s": args.duration,
    "fps": fps,
    "frames": frame_count,
    "resolution": [width, height],
    "camera_azimuth": args.azimuth,
    "speed": 1,
    "poses_sha256": hashlib.sha256(poses_file.read_bytes()).hexdigest(),
    "audio_sha256": hashlib.sha256(audio.read_bytes()).hexdigest(),
    "source_cue_s": args.source_start_seconds,
    "audio_seek_s": seek,
    "recording_start_s": args.start_seconds,
    "estimated_playback_latency_ms": clock["estimated_playback_latency_ms"],
    "alignment": "Recorded monotonic pose times and replay source clock; no fitted beat shift",
    "video_sha256": hashlib.sha256(args.output.read_bytes()).hexdigest(),
}
args.output.with_suffix(".json").write_text(json.dumps(metadata, indent=2) + "\n")
print(json.dumps(metadata, indent=2), flush=True)
