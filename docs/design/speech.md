# On-device speech

This page owns English text-to-speech, its model, playback ownership and beak timing.
The robot synthesizes each request locally with Paradee v1.2's `af_heart` voice. It keeps
Heart's original pitch and timbre. The [demo](../../demos/speech/README.md) owns runnable examples.

## Text becomes audio on the robot

`duck-speech` is a native Rust library. A single background worker in `robotd` loads its
English phonemizer and ONNX model once. Each `robot.say` request follows this path:

1. Validate bounded text and convert it to American Misaki phonemes with `loqui-g2p` 0.4.0.
   Its tokenizer, lexicons, POS tagger and neural fallback for unfamiliar words are embedded.
2. Split at sentence and word boundaries, preserving every phoneme, into blocks of at most
   256 phonemes plus the model's two padding tokens. Unknown phoneme characters are errors;
   they are never silently dropped to make an input fit.
3. Run the pinned INT8 Paradee model with `voice=0` (`af_heart`) and `speed=1.0`. It predicts
   the speech and produces 24 kHz mono PCM. The complete request is synthesized before playback.
4. Pass PCM directly to the existing exclusive audio player. An amplitude envelope derived
   from those exact samples proposes beak targets through the normal servo safety path.

There is no speech WAV bank or intermediate audio file in the daemon path. The optional
WAV output in `sounds speech say --out` is for listening and diagnostics. Inference has no
network dependency, Python interpreter, TTS server or separate daemon. The existing animal
sounds retain their hardware-derived personalities and their existing WAV bank.

The native phonemizer ports Misaki's English rules; its neural fallback can pronounce unusual
words differently from the Python reference's eSpeak fallback. Pronunciation should be checked
for names and wordplay. The acoustic model and selected Heart voice are unchanged.

## Pinned assets and installation

The model is `sahilmahendrakar/Paradee-8M-v1.2` at revision
`ed2934d4df8a4fd891e29584c5e1104649409765`, file `onnx/paradee_int8.onnx`.
Its 8,462,119 bytes have SHA-256
`a6989796692ac173597e15d405c971989202d5c73cfad65f22e90bd7b6c97da1`.
The matching configuration and vocabulary are embedded in `duck-speech/assets/`.

`sounds speech install` downloads that pinned file to
`/var/lib/robot/speech/paradee-v1.2-int8.onnx`. It takes an installation lock, downloads to a
unique adjacent temporary file with a 30-second limit, checks size and SHA-256, syncs and
publishes by rename. A valid installed model is a true no-op. Failed downloads cannot replace
it. The model is checked again before loading; weights are not committed to this repository.

The existing shipped `sounds` binary contains the installer. The release's postinstall hook
runs `sounds speech install --if-enabled /etc/robot/robotd.toml`, using layered configuration.
With speech or master audio disabled it performs no download. An installation failure warns;
optional speech never makes a moving robot unhealthy. The updater's hook timeout is unchanged.
The same installer works before first activation and with `--model PATH` for development.

`[speech]` defaults to `enabled=false`, the model path above and `mouth_offset_ms=0`.
Configuration changes require a `robotd` restart. `audio.enabled` is the master prerequisite.
The model and the legacy quack bank are independent: speech can work without rendered quacks.

The model/configuration use Apache-2.0. The native frontend uses MIT code with attributed
Apache-2.0 language data and neural weights. `sounds speech licenses` prints the bundled notices.
`Cargo.lock` fixes the frontend implementation. Its Rust requirement raises this workspace's
compiler floor to 1.92, within the Yocto ceiling of 1.94.1.

## Control-loop and player ownership

The control loop never performs phonemization, inference, PCM conversion or pipe writes.
The worker uses one ONNX intra-op thread and one inter-op thread; CPU memory arenas and memory
patterns are disabled to reduce retained memory across variable input lengths. On Linux its
thread has lower scheduling priority than the motor loop. Cancellation sets the active ONNX
run's termination flag and is also checked between chunks.

One bounded request slot and one worker operation are permitted. There is no utterance queue.
Every worker request has its own reply channel; canceling it drops that receiver, so a late
result cannot become another request's audio. Text is limited to 512 UTF-8 bytes. Each audio
result must be finite, nonempty and no longer than 30 seconds. Synthesis times out after
20 seconds; playback is bounded by the generated duration plus 1.5 seconds.

`Sound` remains the PCM owner. A live ordinary sound, ride, landing or instrument prevents
speech from starting. Chirps, coos and a released wheee trigger do not interrupt speech.
A newly requested instrument, wheee, alarm, shutdown or unsafe audio state can cancel it,
including while synthesis is pending. Killed player children retire with `try_wait`; no tick
waits for them. A replacement cannot overlap a retiring player. Ordinary replacement one-shots
retain one supersedable pending slot. Cleanup outside regular ticks uses bounded waits.

The generated PCM is sent to `aplay` as raw mono PCM16 at 24 kHz, with a short buffer and a
paced writer thread. The board's configured ALSA plug device handles rate negotiation with its
codec. Playback and the mouth envelope retain the model's sample clock. A successful player
exit counts as `SpeechSpoken` only after the writer delivered all samples; intent acceptance,
model completion and player spawn are not playback completion.

## RPC and beak ownership

API v50 adds typed `robot.say` with `{ "text": "Hello, human." }`, routed and authorized like
`robot.sound`. Install matching daemons, CLI and transports together. `robotctl say TEXT`
prints intent acceptance; `--json` returns the normal `IntentResult`. Disabled speech, invalid
text, a worker that is not ready, busy/unsafe audio and an occupied request slot are immediate
refusals. Safety can change after admission; later refusals and synthesis/player failures are
logged locally.

Only this tick's safety and instrument decisions can start pending audio. Falls, limp-fall,
rest, shutdown and explicit relax cancel speech. Beak proposals additionally require enabled,
ready and fresh servos, a ready IMU, no active skill/instrument and no mode/policy transition.
A ready held robot may animate its beak; driving a locomotion policy is not a prerequisite.

The mouth follows a 50 Hz RMS envelope from the final PCM, indexed by monotonic elapsed time
from player spawn. Positive `mouth_offset_ms` delays it; the allowed correction is ±250 ms.
Spawn is an estimated audio origin, not a DAC timestamp. Skipped ticks jump to current time.
A held nonneutral manual mouth command wins. Any actual numeric command change or denied
servo gate relinquishes speech's mouth ownership for the rest of that utterance. Repeated
identical neutral keepalives do not. Manual takeover updates a held robot's mouth target, so
ending speech cannot close over its grip. Proposed positions use `mouth_target` and the existing
final `safety.apply` path. The effective proposal enters the existing beak-flap tally.

The existing pet classifier keeps its capture stream. Pet starts during pending/playing speech
and a two-second tail after playback are discarded before tallying; events continue to drain.
No second microphone worker or delayed pet response is introduced.

## Verification boundaries

The normal workspace tests require no model download or speech hardware. Opt-in model tests
exercise the real native text frontend, pinned model and ONNX cancellation. The opt-in daemon
test drives actual `robotd --fake --no-policy`, native synthesis and a duration-paced PCM sink,
covering disabled beak suppression, neutral keepalives, manual takeover and completion counters.
The demo guide has their exact commands.

Laptop synthesis and FakeIo are not measurements of RK3566 load, physical speaker latency,
beak calibration or acoustic microphone feedback. Measure those together on the robot under
representative control-loop load before claiming a hardware timing bound.
