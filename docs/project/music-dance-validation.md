# Music dance validation — 2026-11-01

This is development-host evidence, not a physical-robot acceptance report. The
[music-dance design](../design/music-dance.md) owns the behavior; the
[machine-readable results](music-dance-validation.json) hold the measurements. Superseded
stationary/eighth-note experiments remain in Git history rather than this current record.

## Review and cleanup

- A regression reproduced the frozen beat clock during music-loss fading and the retained
  dance-area latch after a completed fade. It passes with both lifecycle fixes.
- Footwork is now tied to the loaded model's identity. A MuJoCo run with an incompatible
  two-network setup remained waiting with `unsupported_walk_policy` and never acquired dance.
- Real ONNX tests restore a missing music model while capture remains open, and give each
  classifier the wrong frontend in turn. A failing consumer leaves the other running.
- The two formerly excluded robotd tests now use a valid model fixture and accept the resolved
  fallback policy path as an actionable load error. Their behavioral assertions remain.
- The harness accepts an explicit `--robotctl` path and rejects nonfinite observation times.
  Python lint/format and development-push shell syntax pass.

The complete 120 BPM lifecycle rehearsal passed acquisition, music-loss fading, reacquisition,
manual takeover/resumption, disabling and persistence across restart. Motors remained disabled
following restart; no fall or pickup was recorded. The observed curved path spanned 22.8 ×
11.1 cm. Head timing error p95 was 24.1 ms; foot timing is included below.

## Workspace verification

Rust 1.94.1 workspace checking, formatting and full-workspace Clippy (`--all-targets`,
`-D warnings`) pass. The unfiltered workspace run passes **1,917 tests**, with 31 ignored and
zero exclusions. All three explicit ONNX parity/recovery/isolation tests also pass with the
local runtime. The earlier two robotd exclusions are no longer needed.

An initial concurrent run hit the existing updater IPC progress test's timing sensitivity
(`missing Swapping`). The final complete run passed without concurrent simulation workloads.
No update-engine logic or timing assertions were changed.

## Audio and model checks

The YAMNet graph and its frontend are unchanged by this review. Source weights and the exported
ONNX/class map remain checksum-pinned, with their Apache license packaged at every release site.
The upstream CSV's CRLF bytes are preserved by Git attributes so checkout retains its checksum.

TensorFlow 2.20.0 / tf-keras 2.20.1 reference inference was compared with the exported ONNX model
on all 521 scores for silence, seeded noise and a multi-tone/impulse input. Maximum absolute
score error was 0.000001014. Rust's feature fixture and explicit ONNX parity test pass.
The pinned exporter previously reproduced the checked-in ONNX bytes exactly.

The generated 16-second audio fixtures produced:

| Reference tempo | Median detected BPM | First beat lock | Phase error p95 |
|---|---:|---:|---:|
| 90 BPM | 89.89 | 3.936 s | 23.85 ms |
| 120 BPM | 120.01 | 3.936 s | 10.06 ms |
| 150 BPM | 150.01 | 3.936 s | 10.05 ms |

Silence, white noise, motor-like hum and rhythmic-click negatives each had zero locked frames.
The TensorFlow tutorial's 4.92-second speech/whistling sample also had zero locks. These are
smoke tests, not a measured music recognition or household false-trigger rate. Debug-build
analysis on this aarch64 development host had roughly 14 ms p99 per 32 ms audio chunk; board
CPU/backlog performance remains unmeasured.

## Physical quarter-note evidence

The baseline recordings at feature implementation `d4df3b6` used actual MuJoCo collision-mesh
landings, not command oscillations. All recordings below alternate feet on quarter notes,
pass clearance and curved-path checks, and report zero falls/pickups. Each observation is
24 seconds after acquisition. The review changes leave the steady stepping waveform intact.

| Input | Expected / actual contacts | Median interval in beats | Coverage / precision | Foot error p95 |
|---|---:|---:|---:|---:|
| Baseline generated 90 BPM | 32 / 32 | 0.993 | 100.0% / 100.0% | 33.9 ms |
| Baseline generated 150 BPM | 54 / 54 | 1.001 | 96.3% / 96.3% | 64.5 ms |
| Baseline MP3 steady passage | 27 / 27 | 0.983 | 96.3% / 96.3% | 63.8 ms |
| Baseline generated 120 BPM | 43 / 43 | 0.996 | 97.7% / 97.7% | 65.2 ms |
| Review generated 120 BPM | 43 / 43 | 0.991 | 97.7% / 97.7% | 64.9 ms |

The older eighth-note recording still fails the current fixed quarter-note scorer: 68 contacts
for 34 quarter-note targets, a median interval of 0.497 beats and 50% precision. The reference
cannot adapt to observed cadence or fit away a phase offset. The 70 ms matching/error guard
includes one 10 ms analysis hop over the earlier 60 ms diagnostic; raw errors are shown above.

## Playback timing and demo

A clean source export with a fresh minimal Python environment and checksum-verified VelStand
policy passed audible replay: 28/28 contacts, 96.4% coverage/precision, 54.5 ms foot error p95,
0.991-beat median interval and 100% alternation. A silent replay retained quarter-note cadence
and movement but failed absolute timing: 55.6% matching and 125.5 ms foot error p95.

Playback scores account for an **estimated 80 ms output delay**; silent runs use the zero-delay
source clock. Neither is an acoustic calibration for another device. This discrepancy is
recorded rather than fitted away or hidden by changing the tolerance.

The [30-second preview](../../demos/music-dance/README.md) contains two different 15-second
simulation takes with a visible camera cut. It preserves recorded pose/audio timing at normal
speed. It is a demonstration, not evidence of uninterrupted recognition across the whole track.

## Pending acceptance

No physical-robot test was possible. Board latency/backlog, microphone gain, servo self-noise,
actual beat-to-motion delay, pickup/fall preemption and longer balance trials remain for delivery.
Other locomotion models need their own profile calibration. A labeled corpus of real music and
household negatives is required before accuracy claims. Activation remains off by default.
No daemon release was published or installed; the published preview is media only.
