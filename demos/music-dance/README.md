# Music dance demo

Watch the real control daemon dance in MuJoCo while hearing the same audio that its simulated
microphone receives. No robot, host microphone, training job or GPU is needed. This feature is
opt-in and has been validated in simulation; physical robot validation is pending.

The daemon alternates feet once per quarter-note beat and follows a compact curved path.
[Behavior and implementation](../../docs/design/music-dance.md) and
[validation results](../../docs/project/music-dance-validation.md) describe its limits.

## 30-second preview

<video src="https://github.com/user-attachments/assets/79454828-8846-43da-bd04-e11e17808aff" controls width="960"></video>

720p, 30 fps: two different 15-second simulation takes joined with a camera cut. Each uses
its recorded motion and soundtrack at normal speed, with no fitted beat shift. The existing
estimated 80 ms playback delay is retained. The clip is labeled as MuJoCo simulation.

## Set up on Linux

Check out the feature branch:

```sh
git clone --branch feature/music-dance https://github.com/mayflower/microduck.git
cd microduck
```

Run these commands from this repository's root. Install Rust 1.94.1, Python 3.12, `uv`, Git,
`curl`, FFmpeg and ALSA's `aplay` first. Rust native prerequisites are listed in
[CONTRIBUTING.md](../../CONTRIBUTING.md). A visible MuJoCo window needs a graphical desktop
with working OpenGL; audible playback needs a configured ALSA output.

Create a separate, pinned simulator checkout and a small CPU Python environment. The source
checkout supplies the robot meshes and body server; the full RL training stack is unnecessary.
Use a fresh directory for this checkout so an existing training environment is not altered.

```sh
git clone https://github.com/pollen-robotics/microduck_rl.git /tmp/microduck-dance-rl
git -C /tmp/microduck-dance-rl checkout bae39f5900b318f7e87fc485c30a31213cfa14eb
uv venv --python 3.12 .venv
uv pip install --python .venv/bin/python -r demos/music-dance/requirements.txt
export PYTHONPATH="/tmp/microduck-dance-rl/src${PYTHONPATH:+:$PYTHONPATH}"
export ORT_DYLIB_PATH="$(.venv/bin/python -c 'import pathlib, onnxruntime; print(next((pathlib.Path(onnxruntime.__file__).parent / "capi").glob("libonnxruntime.so.*")))')"
```

Download the existing VelStand policy at the revision used for this demo. Its checksum also
matches the source policy recorded in `robotd/assets/dance-cycle.json`.

```sh
mkdir -p /tmp/microduck-dance-policies
curl --fail --location --output /tmp/microduck-dance-policies/velstand.onnx \
  https://huggingface.co/pollen-robotics/microduck-policies/resolve/665ae4eb20bfe13bca1ef4ea6d4bb6b4a096e448/velstand.onnx
echo '2fc7f8ee6acd7d0d3f66a99927b5d5a56589aeed02ec714de329b252d5ccf492  /tmp/microduck-dance-policies/velstand.onnx' | sha256sum --check
cargo +1.94.1 build --locked -p robotd -p robotctl
```

## Watch and listen

```sh
.venv/bin/python scripts/test-music-dance-sim.py \
  --body-python .venv/bin/python \
  --policies /tmp/microduck-dance-policies \
  --audio demos/music-dance/hear-me-now.mp3 --start-seconds 80 \
  --viewer --play-audio --demo --observe-seconds 120
```

The viewer stays open during acquisition and the observation period, then closes. The console
prints the temporary artifact directory and acquisition state. Allow for the initial beat lock
and standing preparation before the feet start. `Ctrl+C` stops the demo and its child processes.

`--start-seconds 80` starts in the percussive passage used during development. To hear the whole
track, omit that flag. To run without a window or sound, omit `--viewer --play-audio`. Add
`--check-rhythm` with `--play-audio` to score a steady percussive passage, or omit `--demo` to exercise music loss,
manual interruption, disabling and restart persistence. Output includes daemon logs, body
samples and physical footwork measurements; it is written outside the checkout by default.

Absolute timing scores currently use an estimated 80 ms ALSA output delay from the development
host, recorded in each report. This is not an acoustic calibration for your device. Silent runs
use the source clock with zero output delay and can fail the absolute timing guard even when
cadence, alternation and path checks pass. Use silent runs for movement/lifecycle checks;
audio latency and beat-to-motion timing still require validation on each playback device and
on the physical robot.

On a real robot, [activation and prerequisites](../../docs/design/music-dance.md) apply.
The simulation harness connects only to temporary simulation processes.

## Reproduce the video

With the environment above set up, capture two takes and render 15 seconds from each.
Rendering runs separately from physics and keeps its recorded timing; it does not speed up
the motion or fit an audio shift to the result. The two source cue points give different
acquisition histories and camera views of the dance.

```sh
for cue in 65 80; do
  .venv/bin/python scripts/test-music-dance-sim.py \
    --body-python .venv/bin/python --policies /tmp/microduck-dance-policies \
    --audio demos/music-dance/hear-me-now.mp3 --start-seconds "$cue" \
    --play-audio --demo --observe-seconds 34 --output "/tmp/microduck-video-$cue"
done
MUJOCO_GL=egl .venv/bin/python scripts/render-music-dance-video.py \
  /tmp/microduck-video-65 --audio demos/music-dance/hear-me-now.mp3 \
  --source-start-seconds 65 --start-seconds 2 --duration 15 --azimuth 225 \
  --output /tmp/microduck-take-a.mp4
MUJOCO_GL=egl .venv/bin/python scripts/render-music-dance-video.py \
  /tmp/microduck-video-80 --audio demos/music-dance/hear-me-now.mp3 \
  --source-start-seconds 80 --start-seconds 2 --duration 15 --azimuth 180 \
  --output /tmp/microduck-take-b.mp4
ffmpeg -hide_banner -loglevel error -y \
  -i /tmp/microduck-take-a.mp4 -i /tmp/microduck-take-b.mp4 \
  -filter_complex '[0:v][0:a][1:v][1:a]concat=n=2:v=1:a=1[v][a]' \
  -map '[v]' -map '[a]' -t 30 -r 30 -c:v libx264 -crf 20 -pix_fmt yuv420p \
  -c:a aac -b:a 192k -movflags +faststart /tmp/microduck-music-dance-30s.mp4
```

The renderer needs offscreen OpenGL/EGL and an FFmpeg build with `libx264`, AAC and `drawtext`.
It writes a JSON sidecar containing audio/pose/video checksums, the clock offset, frame count
and camera view. It rejects an interval with stationary head/feet rather than presenting a
musical break as continuous dancing. The MP4 and provenance are hosted as assets of the
[media-only demo prerelease](https://github.com/mayflower/microduck/releases/tag/music-dance-demo-v1),
outside Git history; this is not an installable daemon release.

## Demo recording

`hear-me-now.mp3` is the user-provided `Hear me now.mp3`, included with the contributor's
permission. Duration: 168.440 seconds. SHA-256:
`4b8216ff0d3540da159ad791431dd29c1e6be7c971a5aadaba84dbe8e6079034`.
The repository's Apache-2.0 code license does not grant a license to this recording.
You can substitute any local MP3/WAV using `--audio /path/to/track`.
