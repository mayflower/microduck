# Music dance validation — 2026-11-01

The implementation and its reproducible commands are owned by
[the music-dance design](../design/music-dance.md). This record describes development-host
measurements before the physical robot arrives. [Machine-readable results](music-dance-validation.json)
retain the replay metrics; they are not measurements from a Radxa board.

## Completed checks

- `cargo +1.94.1 check --workspace`: passed, including a final check after the capture changes.
- Workspace tests: 1,896 passed, 29 ignored, with two pre-existing failures explicitly filtered
  after reproducing both against an archive of the unchanged HEAD. Final focused tests across
  the eight affected crates passed 966 tests; the final capture/publishing changes also passed
  the pet-detect and xtask suites. The explicit ONNX parity test passed separately.
- Clippy with `-D warnings`: passed for pet-detect, robotd, duck-ipc-proto and robotd-params,
  including all targets. The broader invocation including robotctl encounters an existing
  `nonminimal_bool` in unchanged `robotctl/src/monitor.rs:2556`.
- Python utility lint/format checks and development-push shell syntax: passed.
- All three release sites package the model, provenance, class map and Apache license;
  xtask tests guard every asset at every site.
- Re-exporting the pinned H5 using the checked-in exporter produces a byte-identical ONNX file:
  SHA-256 `9937cd311979e1b8519e5b9ac58f80931e273acedea705c36112376bc8f7f8af`.

The pre-existing test failures, reproduced without these implementation changes, are
`robotd::tests::setting_a_skill_keeps_what_the_call_left_out` (its deliberately invalid ONNX file
is rejected by the installed runtime), and
`robotd::tests::an_unloadable_policy_holds_the_pose_and_reports_why` (it expects a different
missing-policy path than the default resolved on this host). The test filter names these two
only. Their source and assertions were preserved.

## Audio and numerical checks

TensorFlow 2.20.0 / tf-keras 2.20.1 reference inference was compared with the exported ONNX model
on all 521 scores for silence, seeded noise and a multi-tone/impulse input. Maximum absolute
score error was 0.000001014. Rust's YAMNet frontend is checked against an independently generated
TensorFlow feature fixture. The pet model's different frontend is unchanged.

The checked-in fixture generator and `music-detect` replayed 16-second synthetic tracks:

| Reference tempo | Median detected BPM | First beat lock | Phase error, 95th percentile |
|---|---:|---:|---:|
| 90 BPM | 89.89 | 3.936 s | 23.85 ms |
| 120 BPM | 120.01 | 3.936 s | 10.06 ms |
| 150 BPM | 150.01 | 3.936 s | 10.05 ms |

All tracks retained beat lock after acquisition. Motion adds one second of eligible lock and
then ramps amplitude for a second. Analysis of each 32 ms audio chunk, including occasional
inference, had roughly 14 ms p99 in the final debug-build replay on this aarch64 development
host. This is a host observation, not a board performance budget.

Four generated negatives — silence, white noise, motor-like hum and rhythmic clicks — each ran
for 16 seconds with zero locked frames. The TensorFlow tutorial's 4.92-second speech/whistling
sample also had zero locks. These are smoke checks; they do not establish a household false
trigger rate or a detection rate on real pop, rock or electronic tracks.

## Initial conservative dance in MuJoCo

The checked-in simulation harness used the real robotd, robotctl, capture worker, music model,
pickup model and the local VelStand policy. Its paced WAV capture substituted for the microphone;
no hardware connection, host microphone or speaker was used. Observations included 168 state
frames across ten seconds of dancing:

- Standing policy throughout, zero commanded translation/rotation and no fall.
- Head pitch and yaw stayed within 4° and 8°; neck pitch and head roll stayed zero.
- Switching the capture to silence caused music-loss fading and a return to waiting; restoring
  music reacquired the dance.
- A manual head command interrupted it and it resumed after the idle interval.
- `robotctl dance on`, `off` and `status` worked through the actual CLI and IPC server.
- Enabling survived a daemon restart; the restarted daemon did not enable motor control.

Unit tests additionally cover stale/nonfinite analysis, immediate manual/safety/disable
revocation, bounded body targets, no body-height/velocity command, unchanged idle keepalives,
persistent settings and shutdown during a stalled capture.

## Expressive choreography update — 2026-11-01

The initial sine overlay was replaced by an eight-beat sequence of weight shifts, nod accents,
an upper-body dip and a held pose, mirrored over sixteen beats. Current envelopes and motion
ownership are specified in [music-dance.md](../design/music-dance.md). Target-speed limits and
continuous phrase transitions are covered by nine dance unit tests. The existing manual,
safety, stale-audio, music-loss and persistence checks remain in place. The final robotd run
passed 254 tests across its unit and integration suites, with eight pre-existing ignored tests
and the same two baseline model-test failures filtered as above. Workspace checking with Rust
1.94.1 and robotd Clippy with `-D warnings` passed; the Python harness passed lint and formatting.

The final simulation harness ran the real daemon for 24 seconds of observed dancing at each
of three tempos. A second, read-only simulator connection collected actual trunk orientation,
position and joint angles; it did not change torque or targets. All ranges below are total
peak-to-peak measured motion after discarding the first forty samples, not command amplitudes.

| BPM | Body roll span | Body pitch span | Head pitch span | Head yaw span | Fall / pickup pauses |
|---:|---:|---:|---:|---:|---:|
| 90 | 13.34° | 5.78° | 18.28° | 32.34° | 0 / 0 |
| 120 | 13.26° | 5.66° | 18.04° | 30.71° | 0 / 0 |
| 150 | 13.14° | 5.55° | 17.80° | 28.57° | 0 / 0 |

For comparison, the earlier 120 BPM recording measured 4.85° body roll, 2.72° body pitch,
3.29° head pitch and 16.25° head yaw spans. Thus the increase is present in the simulated
robot's motion, not merely in its requested poses. The largest measured trunk tilt in the
final runs was 7.49°. Every observed tick used the standing policy with zero commanded
locomotion; all three runs passed music-loss fading, reacquisition, manual interruption,
resumption, CLI on/off/status and persisted activation after daemon restart.

A preliminary 6 mm height-dip command produced only 0.76 mm of total height variation.
It was removed: the final dance keeps the height command at zero and expresses the dip with
trunk/head pitch. Developing a visible knee bend or beat-synchronized footsteps remains
separate policy work. No training or policy files changed in this update.

The harness now rejects too-small actual body/head movement and records `motion.json` alongside
the daemon and simulator samples. Its default observation is 24 seconds and can be extended
with `--observe-seconds`; `--viewer` shows the same sequence on the desktop.

## Audible replay support

The demo accepts MP3 through `--audio` (the existing `--wav` option remains an alias) and
optional `--play-audio` speaker output. Both analysis and playback consume the same decoded
PCM blocks with the same pacing and silence transitions. A recording playback sink verified
byte-identical prefixes, including a music-to-silence transition. The full simulation also
passed using a synthetic fixture converted to MP3 and the recording sink. These checks verify
stream identity and lifecycle; device buffering and the physical motion still contribute
latency when a person listens to the actual speakers.

## Per-beat accent correction

Listening to the real track exposed a gap in the range/balance tests: the pose sequence made
physical nods every two or three beats (a 3–3–2 pattern). Its estimated tempo could be correct
while it provided no clear accent on individual kicks. The current generator adds a dominant
single-peak nod and a smaller trunk-pitch pulse on every detected beat, with the pose sequence
reduced to a secondary layer. The pulse continues during base-pose holds and scales down at
fast tempos to preserve timing inside the velocity limits. No BPM was hardcoded for the song.

A separate NumPy/SciPy scorer extracts low-frequency kick-energy peaks from the source and
compares them with physical head-pitch maxima from simulator ground truth. Local parabolic
refinement reduces the effect of the approximately 60 ms observation sampling period.
It measures beat coverage and nearest-event timing, rather than only tempo or range.

| Passage | Reference kicks | Coverage within 90 ms | 95th-percentile absolute offset |
|---|---:|---:|---:|
| Generated 90 BPM | 21 | 100% | 64.7 ms |
| Generated 120 BPM | 28 | 100% | 41.0 ms |
| Generated 150 BPM | 35 | 100% | 26.4 ms |
| Owner-supplied MP3, first steady section | 21 | 100% | 33.1 ms |
| Owner-supplied MP3, middle steady section | 35 | 100% | 38.6 ms |
| Owner-supplied MP3, later steady section | 27 | 100% | 25.4 ms |

These are simulated physical poses against kick-energy peaks, with an estimated 80 ms speaker
clock offset. A local ALSA playback probe measured about 77.45 ms of stream offset, using the
[ALSA PCM timing API](https://www.alsa-project.org/alsa-doc/alsa-lib/pcm.html). This is not an
acoustic loopback measurement; different output devices and the delivered robot need their own
latency calibration. The MP3 and decoded audio stay outside the repository; only score results
are recorded in the validation JSON.

The three generated-tempo runs also passed music-loss fade, reacquisition, manual interruption,
resumption, CLI controls and restart persistence. The final robotd suites passed 255 tests with
the same eight ignored and two pre-existing model-test failures filtered. Workspace checking
with Rust 1.94.1, robotd Clippy with `-D warnings`, and Python lint/format checks passed.

For listening, `--demo` leaves the track uninterrupted instead of injecting test pauses.
`--start-seconds` cues both analysis and playback together; initial acquisition now waits up
to 90 seconds and prints its state, avoiding the earlier 30-second timeout during an intro.
`--check-rhythm` enables the independent physical timing guard for steady percussive passages.
An observation clock snapshot keeps scores reproducible after the capture is restarted.

## Moving dance with footwork

The stationary dance now has an optional walking overlay, enabled by default when dance itself
is opted in. It requests a sixteen-beat figure-eight with beat-accented speed and gentle turns,
using contact odometry to correct local tracking error. No gait weights changed. Experiments
with commands below 0.16 m/s left both feet planted (less than 1 mm clearance and about 6 mm
trunk movement); a 0.38 m/s command ceiling reliably produces steps with the existing VelStand
policy. Physical speed remains below command speed, and the path rounds off the reference.

The MuJoCo checks now reconstruct collision-mesh foot clearance from actual joint/trunk poses.
Both feet must lift repeatedly, reach at least 6 mm clearance, and the path must have measurable
variance perpendicular to its main axis. This catches both the original planted feet and a
straight diagonal walk. The final 24-second runs produced:

| Input | Left/right peak foot clearance | Path width × depth | Nod coverage within 90 ms | Nod error p95 |
|---|---:|---:|---:|---:|
| 90 BPM | 14.6 / 14.6 mm | 22.0 × 16.4 cm | 100.0% | 77.6 ms |
| 150 BPM | 14.4 / 15.1 mm | 22.6 × 14.7 cm | 92.7% | 97.3 ms |
| Owner MP3, middle section | 14.3 / 14.8 mm | 24.8 × 14.6 cm | 100.0% | 58.6 ms |

All three runs passed physical footwork/path checks without falls or pickups. Individual
footfalls retain the policy's cadence; these results do not establish beat-locked landings.
Walking uses only the per-beat head-pitch pulse plus slower yaw gestures: retaining the old
pitch phrase cancelled some fast nods under gait disturbances. The pitch-only refinement
restored the timing guard at 150 BPM. As above, scores use an estimated 80 ms output offset.

Rust 1.94.1 workspace checking and robotd/robotd-params Clippy passed. The final suites passed
408 tests with eight ignored and the same two known pre-existing model tests filtered. The
walking lifecycle run also passed music fade, manual takeover/resumption, disable and restart
persistence. The final head-pitch refinement does not change that lifecycle logic.

## Footfall synchronization correction

The previous walking stage was visibly out of time despite moving and nodding correctly.
Speed accents did not control the gait's natural cadence. The replacement drives a cyclic
component of knee/hip-roll targets from the beat clock, with the live VelStand policy retaining
balance feedback. The profile is a four-harmonic fit to a settled local walking rehearsal;
its mean pose is omitted, and the source ONNX hash is recorded. The local builder reproduces
the checked-in profile exactly. No weights were retrained or uploaded.

The physics sampler now records at 100 Hz. Collision-mesh clearance identifies a lifted foot's
return below 0.5 mm, interpolated between samples. The timing guard uses independent kick
peaks and the declared musical subdivisions, matches contacts one-to-one, and penalizes both
missed accents and extra offbeat contacts. It does not fit a phase offset to observed feet.
The previous free-running MP3 recording fails this guard. A separate guard retains actual
curved movement: at least 15 mm RMS and 30 mm p95 perpendicular deviation from the best-fit
straight path, plus repeated foot lift and a bounded local area. A variance ratio alone was
replaced because it penalized long, curved paths simply for their aspect ratio.

| Input | Steps per beat | Foot coverage | Foot precision | Foot error p95 | Nod coverage | Nod error p95 |
|---|---:|---:|---:|---:|---:|---:|
| Generated 90 BPM | 3 | 99.0% | 100.0% | 45.0 ms | 100.0% | 12.7 ms |
| Generated 150 BPM | 2 | 98.1% | 100.0% | 25.9 ms | 100.0% | 55.1 ms |
| Owner MP3, middle steady passage | 2 | 98.5% | 98.5% | 29.3 ms | 100.0% | 62.2 ms |

All three 24-second recordings pass the final foot timing, head timing, clearance and curvature
checks without falls or pickups. A separate repeat of the complete MP3 lifecycle passed
foot/head timing, music-loss fading, manual takeover/resumption, disabling and restart
persistence with motors remaining disabled after restart. That repeat measured 95.6% foot
coverage, 97.0% precision and 57.1 ms p95 error, showing some run-to-run acquisition variation.
Stepping at 123 BPM uses the beat and eighth-note offbeat;
90 BPM uses triplets to stay near the balance gait's supported cadence. Subdivision choice is
frozen for each dance, avoiding parity flips when the estimated tempo changes slightly.

Head pitch is a bounded direct motor target during this walking overlay, replacing that joint's
policy target rather than double-adding a command. This prevents balance corrections from
cancelling fast nods. The policy continues to observe the measured head and controls the other
head joints. The runtime tests passed 411 tests, eight ignored, with the same two pre-existing
model failures filtered. Rust 1.94.1 workspace check, robotd/robotd-params Clippy and Python
lint/format checks passed. The measured speaker clock remains estimated, as described above.

## Quarter-note cadence correction

The current implementation targets one alternating footfall per detected beat; earlier
walking sections above are development history, including the eighth-note version rejected
by the owner. The mechanism is owned by [the dance design](../design/music-dance.md).

The previous eighth-note recording has 68 contacts for 34 quarter-note targets, a median
interval of 0.497 beats and only 50% precision under the quarter-note reference. The updated
scorer rejects it. Its reference is fixed to quarter notes and cannot adapt to the observed
cadence or fit away a phase offset. It allows unmatched events, so one missing contact does
not force the rest of the assignment onto the wrong beats; early/late boundary contacts are
included within the matching tolerance.

The final physical recordings, with one-quarter-note matching and alternating feet:

| Input | Expected / actual contacts | Median interval in beats | Coverage / precision | Foot error p95 |
|---|---:|---:|---:|---:|
| Generated 90 BPM | 32 / 32 | 0.993 | 100.0% / 100.0% | 33.9 ms |
| Generated 150 BPM | 54 / 54 | 1.001 | 96.3% / 96.3% | 64.5 ms |
| Owner MP3, middle steady passage | 27 / 27 | 0.983 | 96.3% / 96.3% | 63.8 ms |
| Generated 120 BPM | 43 / 43 | 0.996 | 97.7% / 97.7% | 65.2 ms |

All four recordings pass head timing, foot clearance, quarter-note cadence, alternating-foot
and curved-path checks without falls or pickups. Quarter-note matching uses a 70 ms window
and p95 limit, adding one 10 ms analysis hop to the earlier 60 ms diagnostic; the raw measured
errors are shown above. Coverage and precision must each reach 90%, with 90% alternating feet
and a median interval of 0.85–1.15 beats. Curvature checks use 10 mm RMS and 20 mm p95 deviation
from a best-fit line for the smaller, slower stepping path. These values continue to reject
planted feet and straight walking. Speaker timing retains the estimated 80 ms offset.

A separate complete 120 BPM run passed music-loss fading, reacquisition, manual interruption/
resumption, disabling and restart persistence; torque stays disabled after restart. The final
Rust suites passed 420 tests, with ten ignored and the same two known pre-existing model tests
filtered. Rust 1.94.1 workspace checking, robotd/odometry/robotd-params Clippy and Python lint/
format checks passed. No policy weights or RL files were changed, and no release was published.

## Feature branch publication checks — 2026-11-01

The focused feature tree passes Rust 1.94.1 workspace checking and formatting. The complete
workspace test run passes 1,913 tests, with 29 ignored and the same two pre-existing failures
named above filtered explicitly. The YAMNet ONNX/TensorFlow parity test passes separately.
Staged model/class-map hashes match their provenance; Git preserves the upstream CSV's CRLF
bytes so checkout does not invalidate its checksum.

The demo was exercised from an export of the staged files, with rebuilt daemon/CLI binaries,
a clean simulator source archive at `bae39f5900b318f7e87fc485c30a31213cfa14eb`, a fresh minimal
Python environment and a freshly downloaded, checksum-verified VelStand policy. It does not
need the training environment. The included MP3 was cued at 80 seconds and observed for 24
seconds after acquisition. With speaker playback, the run passes head timing, physical foot
clearance, quarter-note timing, alternation and curved-path checks: 28/28 contacts, 96.4%
coverage/precision, 54.5 ms foot error p95, 0.991-beat median interval and 100% alternation.
The path spans 16.1 × 9.6 cm with no fall or pickup.

A silent run passed movement checks and produced 27/27 contacts with a 0.992-beat median
interval and 100% alternation, but **failed absolute timing**: 55.6% matching and 125.5 ms
foot error p95 against the zero-delay source clock. Playback reports instead account for an
estimated 80 ms output delay. This difference is recorded, not fitted away or hidden by a
looser guard. The demo guide now distinguishes silent movement/lifecycle checks from audible
timing checks. Acoustic latency is still uncalibrated outside the development playback setup.

## Pending acceptance

No physical-robot test was possible. Board CPU/backlog, microphone gain, self-noise, actual
beat-to-motion delay, pickup/fall preemption and longer balance trials remain for delivery.
A labeled corpus of real music and household negatives is also required before making accuracy
claims. Default activation remains off, and no release was published.
