# Speak with Heart on the microduck

Speech is synthesized on the robot from newly requested text. Install the pinned model once:

```sh
sudo /opt/robot/daemon/current/bin/sounds speech install
sudo robotctl configure
```

Enable `speech.enabled`, leave `audio.enabled` on and accept the offered `robotd` restart.
Then speak:

```sh
robotctl say "Hello, human. Duck reporting for duty."
robotctl say "What shall we do today?" --json
```

The response acknowledges an intent. The control loop can still refuse playback if a higher
priority owner or unsafe state intervenes. Completion is logged as `SpeechSpoken`.
The [speech design](../../docs/design/speech.md) owns limits, model identity and beak timing.

## Listen and develop without a robot

Build the same native binaries used on the robot:

```sh
cargo +1.94.1 build -p sounds -p robotd -p robotctl
./target/debug/sounds speech install --model /tmp/paradee-v1.2-int8.onnx
./target/debug/sounds speech say "This sentence is synthesized locally." \
  --model /tmp/paradee-v1.2-int8.onnx --device default
```

Only installation downloads the model. These commands use no Python or external phonemizer.
ONNX Runtime must be installed; `ORT_DYLIB_PATH` can name its shared library.
Add `--out /tmp/heart.wav` to the `say` command to save a listening sample.
`sounds speech phonemes TEXT --model PATH` inspects pronunciation.

To exercise the daemon and CLI, create parameters and run:

```sh
cat > /tmp/speech-demo.toml <<'TOML'
[audio]
device = "default"
greet = false
pet_detect = false
[speech]
enabled = true
model = "/tmp/paradee-v1.2-int8.onnx"
[pickup]
enabled = false
TOML
./target/debug/robotd --fake --no-policy --socket /tmp/speech-demo.sock \
  --params /tmp/speech-demo.toml
```

In another terminal, once the journal says `on-device speech ready`:

```sh
./target/debug/robotctl --robot-socket /tmp/speech-demo.sock say "Hello from my own CPU."
```

FakeIo has no physical beak or speaker timing guarantee. Stop the daemon with Ctrl-C.

## Run the real-model checks

The model is deliberately absent from Git and from ordinary offline CI tests. After installing it:

```sh
export MICRODUCK_SPEECH_MODEL=/tmp/paradee-v1.2-int8.onnx
cargo +1.94.1 test -p duck-speech -- --ignored
cargo +1.94.1 test -p robotd --test speech -- --ignored
```

For a reusable listening package, provide a JSON array of `{ "id": "sample_00", "text": "Hello." }`
objects, then run:

```sh
cargo +1.94.1 run --release -p duck-speech --example audition -- \
  /tmp/paradee-v1.2-int8.onnx cases.json /tmp/heart-audition
```

That writes native 24 kHz WAVs and a JSON record of the actual input phonemes, durations and
synthesis times. Listening files are test artifacts; the daemon streams PCM directly.
