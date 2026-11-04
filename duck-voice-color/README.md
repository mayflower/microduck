# Voice color utility

A separate native GPL-3.0-or-later audio utility. [The speech design](../docs/design/speech.md)
owns the nasal Heart preset and its runtime integration. Raw mono f32le PCM enters stdin and
leaves stdout with the same number of samples. Pitch and duration ratios are one.

Build from the source included in a release:

```sh
cd source
cargo build --release -p duck-voice-color
```

Needs Rust 1.92–1.94.1 and a C++17 compiler. No model, Python, system Rubber Band installation
or external audio tool is required. `--version` reports the processor contract; `--licenses`
prints its notices. `--sample-rate`, `--formant` and `--loudness` permit independent PCM use.

The unmodified Rubber Band 4.0.0 sources and its original licenses are under `vendor/rubberband/`.
This executable's sources are GPL-3.0-or-later; the surrounding runtime has its own license.
