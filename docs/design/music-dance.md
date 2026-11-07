# Autonomous dance to music

[Run the included MuJoCo demo](../../demos/music-dance/README.md) for a pinned simulator setup
and audible playback without a robot.

Enable it once on the robot:

```sh
robotctl dance on
robotctl dance status
robotctl dance off
```

Activation is off by default. `on` and `off` write `dance.enabled` through the existing
configuration editor before acknowledging success. On an installed robot this is the owner's
persistent layer, so activation survives daemon restarts, releases and OS updates. A custom
`--params` file is edited in place. The command grants permission to dance while idle; it never
enables torque, starts the policy, stands up a seated robot or changes the selected gait.

The robot must already be upright under its walking/standing controller in walking mode,
with audio and pickup detection available. By default `dance.footwork = true`: the existing
walking policy follows a beat-driven local path with clocked footfalls while the head keeps its per-beat nod. The
shipped VelStand gait is used without replacing or retraining it. Set `dance.footwork = false`
and restart robotd to retain the stationary body/head dance instead.

## Ownership and motion

`robotd/src/dance.rs` is a pure command generator. The control loop applies its output only after
its normal power, policy, skill and safety decisions. Eligibility requires a fresh sensor read,
a settled IMU, an enabled walking/standing controller, a loaded pickup detector, and no fall, pickup pause,
body-pose client, homing, policy swap, shutdown, sitting, active skill, theremin or chorale.
Roller mode cannot dance. The autonomous overlay never writes into shared manual intents.

Changed head targets, nonzero drive commands and discrete control actions interrupt the dance.
The last manual activity must be at least ten seconds old before acquisition can resume. Zero
velocity, unchanged head and inactive body-pose keepalives do not keep claiming manual ownership.
Holding a velocity or body pose does. Ordinary idle glances and breathing give way to the dance.
The normal safety clamps and selected balance/walking policy remain authoritative.

The gesture layer follows an eight-beat phrase: four alternating lateral weight shifts with
opposing head sweeps, nod poses, a forward upper-body dip and a held base pose. The next phrase
mirrors roll and gaze, making a sixteen-beat cycle. These phrases count detected beats; they
do not infer the song's downbeat or bars. Quintic interpolation joins the gesture poses, with
short holds on arrivals and a longer gaze/body hold at the phrase end.

A dominant head nod and smaller trunk-pitch accent occur on **every** detected beat, including
while a gesture pose is held. The pulse is `(cos(2πb) + 0.15 cos(4πb)) / 1.15`, with its single
maximum at each integer beat. The slower phrase contributes only 25% of head pitch and 50% of
trunk pitch. Above 120 BPM pulse excursion scales by `120/BPM` so rapid music retains the
accent's timing within the speed limit. Without this separate pulse, the original pose sequence
produced nods in a 3–3–2 pattern and looked rhythmically unrelated despite estimating tempo.

With footwork enabled, `robotd/src/dance_motion.rs` follows a sixteen-beat figure-eight
reference with forward/back and lateral motion and heading changes. Contact odometry feeds
position and heading error back into the command. The reference has 20 cm forward and 15 cm
lateral radii; the actual path is rounded and biased by the balance gait. Commands are bounded
to 0.38 m/s and 0.8 rad/s, with translation slewed at 0.9 m/s² and rotation at 4 rad/s² during footwork.
The position correction is 2/s and heading oscillates ±0.6 rad around the starting orientation.

The cadence is **one alternating footfall per quarter-note beat** at every tempo. It never
adds offbeat, triplet or sixteenth-note steps to keep up with the walking policy's natural
cadence. The same foot repeats every two beats; the head continues to nod once per beat.

`robotd/src/dance_gait.rs` uses a short swing (195 ms), followed
by planted standing until the next quarter note. The head ramp finishes in planted standing;
the gait then joins a complete swing from its start, rather than fading into a partial one.
Status remains `acquiring` during that preparation. It traverses half of a recorded leg cycle during each swing. Subtracting
the line between that half-cycle's endpoints makes the motor offset zero before and after
the swing. This lets the live balance controller settle instead of filling the gap with its
free-running walking cadence. During the planted interval velocity is exactly zero and the
controller clears previous-action gait carryover, retaining measured joint/IMU feedback.
This change is confined to the autonomous dance; manual walking retains its normal feedback.

Knee forcing has strength 1.1 and hip-roll forcing 2.25. The profile begins at cycle phase
0.4 and its swing clock lags the beat clock by 0.04 beats. Those are physical-response
calibrations for this simulator, not a hardcoded song tempo. Path velocity is requested only
in the swing window, at 20% of the translation demand. Turn strength tapers from 100% at
90 BPM to 20% at 138 BPM and above, leaving more balance headroom at fast tempos. A
velocity-slew tail cannot continue into the planted pause. The policy retains mean stance,
hip pitch, ankles, hip yaw and live balance feedback.

Foot timing also has sensor feedback. `Odometry::foot_clearance()` estimates relative sole
height from the existing FK and IMU sample, with the lower foot defining the floor. A lift
above 4 mm followed by a return below 1 mm supplies an interpolated landing event. This is
kinematic feedback on a flat floor, not a pressure/contact sensor. The controller measures
the actual once-per-beat head-pitch peaks to account for actuator delay, then aligns the
landings 15 ms after that physical pulse. Head commands alone were an inaccurate timing reference.
Both the head and the feet are still scored independently against music in the simulator.

The foot-phase loop acquires with gain 0.5 and a maximum correction of 0.06 beats per landing;
after lock it tracks opposite-foot averaged errors with gain 0.35, a 10 ms deadband and a 0.03-beat correction cap. Total correction stays
within ±0.4 beats. Corrections happen only inside the planted interval, at least 0.08 beats
from its edges, so a clock adjustment cannot restart a half-finished swing. `acquiring`
continues until the physical head reference is available and three consecutive quarter-note
contacts are within 30 ms of it. Manual/safety interruption clears the phase history.

`robotd/assets/dance-cycle.json` contains four harmonics of a settled, forward VelStand cycle,
without its DC component, plus the source policy hash. Regenerate it locally with:

```sh
../microduck_rl/.venv/bin/python scripts/build-music-dance-cycle.py \
  --policy ../models/policies/velstand.onnx --output /tmp/dance-cycle.json
```

The builder uses the same 50 Hz control and 200 Hz MuJoCo scene as the runtime rehearsal.
It fits settled left-contact cycles, leaves out the mean pose and records no user audio. No
policy weights were retrained. This profile was calibrated with VelStand; other gaits and the
actual motors/floor need their own validation.

Footwork is enabled only when the loaded walking network's SHA-256 matches this profile's
`policy_sha256` and no separate standing network is installed. The check uses the loaded bytes,
so renaming a model does not change compatibility, and swapping a policy updates the decision.
An incompatible setup waits with `unsupported_walk_policy`; it never receives the recorded leg
offsets. `dance.footwork = false` selects the stationary gesture mode instead.

Walking uses neutral body-pose commands. Its head-pitch command is a pure 10.5° × min(1,120/BPM)³
cosine pulse on each beat, and gaze follows a slower eight-beat yaw sweep. Rapid lateral
head flips could provoke extra balance steps. During walking,
that joint's motor target is replaced by nominal head pitch plus this bounded pulse, rather
than adding it to the policy's head target. The remaining head joints retain policy control.
With footwork disabled the original standing body/head sequence stays in use. Both modes
bound head pitch to ±12° and yaw to ±16°, with zero neck pitch, head roll and body-height
commands. Crossing a measured 50 cm radius from the initial dance position latches
locomotion off with reason `dance_area_limit`; head gestures continue. Disabling/re-enabling
or a manual/safety interruption resets that anchor and latch. Completing a music-loss fade
also resets them, so the next acquisition starts a new path at the current location. This is a local odometry bound,
not obstacle detection or a guarantee against long-term odometry drift.

Amplitude ramps over one second. A final slew limiter bounds head commands to 140°/s, body
roll to 70°/s and body pitch to 50°/s, including phase corrections and mirrored phrase boundaries.
The phase tracker follows the estimated beat rather than restarting on each analysis result.
Beat/music loss fades to neutral over one second, continuing the last known beat clock so a
partial swing does not freeze during the fade. Manual control, ineligibility, disabling and
missing/stale audio revoke the overlay immediately; they bypass the slew limiter so another
owner takes control on the next tick. Audio older than 500 ms cannot drive it. Active dancing
counts as motion for the updater's restart gate.

## Audio path and recognition

`pet-detect::worker` owns one `arecord` subprocess at mono PCM16, 16 kHz. Optional pet detection,
the ambient sentry and music analysis share that stream. Music works with `audio.pet_detect =
false`; a failed pet classifier does not disable it. When both petting and dance are off the
worker parks without opening the capture device. The music model is loaded lazily on its worker
thread. No FFT, ONNX inference, process I/O or configuration write runs in the motor tick.
A lock-free latest-value slot publishes analysis results; intermediate results are not queued.
Model-load failures retain their diagnostic and retry every five seconds, including while
petting keeps the shared capture alive. Recovering music does not restart that capture.
Music inference failures also clear and retry that consumer without interrupting petting.
A petting inference failure disables that classifier until restart, ending any active petting
event, while music and capture continue.

The capture pipe is nonblocking. Shutdown can interrupt a stalled capture, which is restarted
with bounded backoff. Timestamps follow the captured sample count rather than inference
completion. A backlog over 500 ms is discarded by restarting capture, and every capture restart
clears analysis history and the published result. Toggle transitions restart the capture too,
so enabling never reuses a previous listening session's music lock.

`pet-detect/src/music.rs` has a separate YAMNet frontend: 400-sample periodic Hann, 160-sample
hop, 512-point magnitude FFT, 64 HTK mel bands from 125 to 7500 Hz, log offset 0.001, and
96 time-major frames per patch. This deliberately does not reuse the pet model's 40-band power
features. The pinned YAMNet v1 ONNX graph accepts `[1,96,64]` and emits 521 sigmoid scores.
Music is AudioSet index 132. Six consecutive patches scoring at least 0.6 establish music;
a lower score clears it. Inference is every 7680 samples, using one CPU thread.

The independent onset detector uses positive spectral flux with a 10 ms hop, up to eight
seconds of history, autocorrelation peaks in the 70–160 BPM range, fractional peak refinement
and recent-onset phase alignment. An octave correction needs evidence for the faster pulse,
which avoids interpreting alternating kick/snare accents as half tempo. Confidence must reach
0.55 with about four seconds of onset history. The motion generator then requires a further
second of eligible lock. Clear steady music starts motion at roughly five seconds; manual
activity and other owners can delay that. These thresholds favor clear pop/rock/electronic
beats; they do not promise recognition of every track, quiet passage or syncopated rhythm.

## Wire contract and installation

API version 51 adds `robot.dance` and `robot.dance.set {"enabled":true|false}`. Both are slow-lane
robotd calls. The setter returns `IntentResult`, including a refusal if saving fails. The read
returns `enabled`, `state`, optional `bpm` and optional `reason`. States are `disabled`, `waiting`,
`acquiring` and `dancing`; waiting reasons include `initializing`, `robot_not_ready`,
`manual_control`, `audio_unavailable`, `no_music_or_beat`, `unsupported_walk_policy` and `dance_area_limit`. A fading dance still reports
`dancing` with its loss reason. `robot_not_ready` includes an unavailable pickup detector.

The setter is classified as mutating. BLE routes it after the existing PIN gate; WebRTC permits
it under that transport's existing watching-peer permission model, like other persistent robot
settings. Install matching robotd, robotctl and transport binaries together. All three release
packaging paths include the model, class map, provenance and Apache license. Nothing was
published as a daemon release or installed onto a robot as part of development.

`dance.model` defaults to `/opt/robot/daemon/current/models/music_yamnet.onnx`. Changing model
paths through `robotctl configure` needs a robotd restart; `robotctl dance on|off` applies its
activation live. `audio.enabled = false` or an unavailable model leaves activation waiting
without movement.

## Reproduce the checks without a robot

Use Rust 1.94.1 and point `ORT_DYLIB_PATH` to ONNX Runtime >=1.23. From the runtime checkout:

```sh
cargo +1.94.1 build -p pet-detect --bin music-detect
python scripts/music-dance-fixtures.py /tmp/dance-audio --analyzer target/debug/music-detect
cargo +1.94.1 test -p pet-detect exported_model_matches_tensorflow_reference -- --ignored
```

The fixture generator needs NumPy. It checks deterministic music at 90, 120 and 150 BPM,
acquisition, tempo and phase, and rejects silence, white noise, motor-like hum and clicks.
`/tmp/dance-audio/report.json` records results and analysis timing. Additional negative WAVs in
that folder are checked too; inputs must be mono 16 kHz PCM16. For an individual recording:

```sh
target/debug/music-detect recording.wav --model pet-detect/models/music_yamnet.onnx
```

Stdout is analysis NDJSON; stderr is a timing/acquisition summary. `--features file.f32` dumps
the first log-mel patch for parity checks. This tool consumes a file and opens no microphone.

For the complete daemon, capture worker, policy and MuJoCo body, with local policies available:

```sh
cargo +1.94.1 build -p robotd -p robotctl
python scripts/test-music-dance-sim.py --wav /tmp/dance-audio/music-120.wav
```

Add `--viewer` to watch the same dance sequence in a local MuJoCo window. Run it from a
graphical desktop session. The demo automatically exercises pauses and resumes, then closes
its processes; the default test run has no window and records no video.

To judge timing against audible music, supply your own track and enable playback:

```sh
python scripts/test-music-dance-sim.py --viewer --play-audio --demo --audio /path/to/song.mp3
```

`--demo` observes the track without injecting the test's artificial silence/manual-control
sequence. `--start-seconds` cues both playback and analysis to the same source position.
Initial acquisition allows 90 seconds by default (`--acquire-timeout`), because an intro may
precede a reliably trackable beat. Waiting state/reason changes are printed while acquiring.

`--audio` also accepts WAV and other FFmpeg-readable formats; `--wav` remains an alias.
FFmpeg decodes the source once to mono 16 kHz PCM16. `music-dance-replay.py` paces that single
stream to both robotd's simulated microphone and `aplay` on the host. Pauses silence both
outputs together, and each capture restart restarts both at the same source position. The
speaker device has its own small output buffer, so this is a common source/sample clock,
not a guarantee of zero acoustic latency. Playback errors are reported through `playback.log`
and `playback-error` rather than silently running a mute demo. Default runs remain silent.

The simulator needs the neighboring `microduck_rl/.venv` and `models/policies` checkouts, or
explicit `--body-python` and `--policies` paths. It starts only temporary simulation processes,
substitutes a paced WAV stream for `arecord`, observes walking balance and bounded velocity/head
commands, checks music-loss fading, manual interruption/resumption, disabling and persistence
across restart. The default observation lasts 24 seconds; `--observe-seconds` extends it.
A read-only simulator connection additionally records ground-truth trunk position, orientation
and joint angles, so tests check actual movement rather than only larger command values.
`motion.json` reports measured body/head ranges, height variation, tilt and fall/pickup counts;
`states.json` and `body-states.json` retain the corresponding samples. Measured head spans and movement on both path axes guard against a policy ignoring the
choreography. `score-music-dance-footwork.py` reconstructs the actual collision meshes from
saved joint/trunk poses sampled at 100 Hz and requires repeated lifts of both feet, at least
6 mm peak clearance, and path variance perpendicular to its principal axis (at least 10 mm
RMS and 20 mm at the 95th percentile from its best-fit straight line for the compact quarter-note path). This rejects planted feet
and diagonal straight-line walking. It saves clearances, landing times and path statistics to
`footwork.json`. With `--check-rhythm`, it additionally compares interpolated sole landings to quarter-note
beats using the independent kick reference. A one-to-one match must cover at
least 90% of expected steps and 90% of actual contacts, with p95 error at most 70 ms and at
least 90% alternating feet. Matching cannot fit an arbitrary phase offset to the feet. The
previous free-running and eighth-note demos fail this check even though it passes the physical-motion checks.
Contact events require clearance above 4 mm followed by a descent below 0.5 mm, avoiding
planted-foot angle changes and repeated contact chatter. The same estimated speaker clock
qualification applies to feet and head.
`--check-rhythm` additionally scores measured physical nods against independently extracted
40–180 Hz kick-energy peaks. It requires a steady percussive passage, at least 85% of reference
kicks within 90 ms of a physical nod, and a 95th-percentile absolute error of at most 100 ms.
The scorer is `scripts/score-music-dance-timing.py` (NumPy/SciPy, run with the RL environment).
It accounts for looping fixtures and uses a frozen observation clock, so later capture restarts
do not invalidate the recording's timing. `rhythm.json` records coverage, offsets and reference/
nod times. Playback timing currently uses an estimated 80 ms device offset; an ALSA probe on the
development host measured about 77 ms. These metrics compare physical poses to kick-energy
peaks with an estimated output clock, not calibrated acoustic measurements or general-purpose
beat annotations. The microphone/controller timing on the delivered robot still needs calibration.

All logs and state frames are saved in the printed temporary directory. The harness never
connects to a real robot or uses the host microphone. Only `--play-audio` uses the host speaker.

## Model provenance and parity

`pet-detect/models/music_yamnet.json` pins the upstream revision, source H5 hash, ONNX hash,
class-map hash and frontend contract. The source weights are Google's
[YAMNet v1 weights](https://storage.googleapis.com/audioset/yamnet.h5); the reference architecture,
features and class map come from [TensorFlow's YAMNet implementation](https://github.com/tensorflow/models/tree/master/research/audioset/yamnet).
The Apache-2.0 license is shipped alongside the artifact. The exporter transposes the original
weights into equivalent PyTorch convolution, batchnorm and dense layers, preserving TensorFlow's
SAME padding, then exports ONNX opset 17. No fine-tuning is performed.

To rebuild, download the H5 named in the metadata and use Python 3.12 with torch 2.9.1,
h5py, NumPy and ONNX:

```sh
python scripts/export-music-yamnet.py yamnet.h5 pet-detect/models/music_yamnet.onnx
```

For an independent comparison, download `yamnet.py`, `features.py` and `params.py` from the
metadata's exact `tensorflow/models` revision into a reference directory. In an environment
with TensorFlow 2.20.0, tf-keras 2.20.1, NumPy and onnxruntime:

```sh
python scripts/validate-music-yamnet.py reference-directory yamnet.h5
```

The script checks source/artifact hashes and all 521 scores on silence, seeded noise and a
PCM-quantized multi-tone/impulse input against the original TensorFlow model. Rust tests compare
features to the independently generated TensorFlow fixture, and an explicit ONNX test checks
its full score vector. The fixtures and numerical tolerance prevent a plausible-looking but
incompatible frontend from silently reaching the robot.

## What still needs the delivered robot

The synthetic corpus and short simulation are functional checks, not a measured recognition
rate across commercial music or a hardware safety certification. Before normal use, measure
Radxa inference latency and audio backlog, mic sensitivity and servo self-noise, beat-to-motion
phase delay, fall/pickup preemption and sustained VelStand balance on the actual floor. Keep the
conservative amplitudes until those measurements support a change. A longer labeled corpus of
real music, speech, TV and household sounds is needed before claiming detection/false-trigger
rates. [The validation record](../project/music-dance-validation.md) distinguishes measurements
already made from that later acceptance work.
