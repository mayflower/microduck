#!/usr/bin/env python3
"""Synthetic music at 90/120/150 BPM and negative audio; requires numpy."""

import argparse
import json
import wave
from pathlib import Path

import numpy as np

parser = argparse.ArgumentParser(
    description="Generate deterministic smoke-test audio, not a music accuracy corpus"
)
parser.add_argument("directory", type=Path)
parser.add_argument(
    "--analyzer", type=Path, help="Run checks through the built music-detect binary"
)
args = parser.parse_args()
root = args.directory
root.mkdir(parents=True, exist_ok=True)


def save(name, x):
    with wave.open(str(root / (name + ".wav")), "wb") as f:
        f.setparams((1, 2, 16000, 0, "NONE", ""))
        f.writeframes((np.clip(x, -0.99, 0.99) * 32767).astype("<i2").tobytes())


n = 16 * 16000
t = np.arange(n) / 16000
rng = np.random.default_rng(44)
for bpm in [90, 120, 150]:
    beat = t * bpm / 60
    phase = (beat % 1) * 60 / bpm
    bar = (beat // 4).astype(int) % 4
    chord = np.zeros(n)
    for b, notes in enumerate(
        [
            [261.63, 329.63, 392],
            [196, 246.94, 293.66],
            [220, 261.63, 329.63],
            [174.61, 220, 261.63],
        ]
    ):
        for hz in notes:
            chord += (
                (bar == b)
                * (
                    np.sin(2 * np.pi * hz * t)
                    + 0.35 * np.sin(4 * np.pi * hz * t)
                    + 0.15 * np.sin(6 * np.pi * hz * t)
                )
                * 0.04
                * np.exp(-((beat * 2) % 1) * 2)
            )
    kick = (
        0.45
        * np.sin(2 * np.pi * (45 * phase + 6 * (1 - np.exp(-phase * 25))))
        * np.exp(-phase * 25)
    )
    snare = (
        0.15 * rng.normal(size=n) * np.exp(-phase * 35) * ((beat // 1).astype(int) % 2)
    )
    hat = 0.04 * rng.normal(size=n) * np.exp(-((beat * 2) % 1) * 25)
    bass = (
        0.12
        * np.sin(2 * np.pi * np.take([65.406, 49, 55, 43.65], bar) * t)
        * np.exp(-phase * 5)
    )
    save("music-" + str(bpm), chord + kick + snare + hat + bass)
save("silence", np.zeros(n))
save("noise", rng.normal(0, 0.05, n))
save("motor", 0.05 * np.sin(2 * np.pi * 100 * t) + rng.normal(0, 0.005, n))
save("clicks", 0.5 * rng.normal(size=n) * np.exp(-((t * 2) % 1) * 100))

if args.analyzer:
    import statistics
    import subprocess

    report = []
    for path in sorted(root.glob("*.wav")):
        process = subprocess.run(
            [str(args.analyzer.resolve()), str(path.resolve())],
            check=True,
            text=True,
            capture_output=True,
        )
        rows = [json.loads(line) for line in process.stdout.splitlines()]
        locked = [r for r in rows if r["locked"]]
        item = {
            "case": path.stem,
            **json.loads(process.stderr),
            "locked_frames": len(locked),
            "total_frames": len(rows),
        }
        if path.stem.startswith("music-"):
            bpm = float(path.stem.split("-")[1])
            assert locked, f"{path}: no lock"
            assert 3.8 <= item["first_lock_s"] <= 6, f"{path}: slow acquisition"
            median = statistics.median(r["bpm"] for r in locked)
            assert abs(median - bpm) < bpm * 0.05, f"{path}: wrong BPM {median}"
            errors = sorted(
                abs((r["beat"] - r["seconds"] * bpm / 60 + 0.5) % 1 - 0.5)
                * 60
                / bpm
                * 1000
                for r in locked
            )
            phase95 = errors[int((len(errors) - 1) * 0.95)]
            assert phase95 < 80, f"{path}: unstable phase {phase95}ms"
            item.update(median_bpm=median, phase_error_p95_ms=phase95)
        else:
            assert not locked, f"{path}: false music trigger"
        report.append(item)
    (root / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))
