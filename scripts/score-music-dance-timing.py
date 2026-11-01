#!/usr/bin/env python3
"""Score physical nods against an independent kick reference in a steady music passage.

Requires NumPy/SciPy (available in the neighboring RL environment). This is a
kick-based diagnostic, not a general beat annotation tool for arbitrary genres.
"""

import argparse
import json
import math
import wave
from pathlib import Path

import numpy as np
from scipy.signal import butter, find_peaks, sosfiltfilt


def score(directory):
    clock_file = directory / "observation-audio-clock.json"
    if not clock_file.exists():
        clock_file = directory / "audio-clock.json"
    clock = json.loads(clock_file.read_text())
    bodies = json.loads((directory / "body-states.json").read_text())
    if len(bodies) < 20:
        raise ValueError("too few body samples to score rhythm")
    t = np.array(
        [
            (r["t_ns"] - clock["source_start_monotonic_ns"]) / 1e9
            - clock["estimated_playback_latency_ms"] / 1000
            for r in bodies
        ]
    )
    pitch = np.array([r["positions"][6] for r in bodies])
    with wave.open(str(directory / "audio.wav")) as wav:
        samples = (
            np.frombuffer(wav.readframes(wav.getnframes()), "<i2").astype(float) / 32768
        )
    needed = math.ceil((max(0, t[-1]) + 1) * 16000)
    if len(samples) < needed:
        samples = np.resize(samples, needed)
    # Independent of robotd's broadband spectral-flux tempo/phase estimate.
    low = sosfiltfilt(
        butter(3, [40, 180], fs=16000, btype="bandpass", output="sos"), samples
    )
    energy = np.sqrt(
        np.mean(low[: len(low) // 160 * 160].reshape(-1, 160) ** 2, axis=1)
    )
    section = energy[max(0, int(t[0] * 100)) : int(t[-1] * 100) + 1]
    peaks, _ = find_peaks(
        energy, distance=30, prominence=max(0.005, np.percentile(section, 95) * 0.22)
    )
    beats = peaks * 0.01
    beats = beats[(beats > t[0] + 1) & (beats < t[-1] - 1)]
    delta = np.diff(beats)
    intervals = delta[(delta >= 60 / 160) & (delta <= 60 / 70)]
    if len(intervals) < 8:
        raise ValueError(
            "too few steady kick intervals; choose a regular percussive passage"
        )
    hist, edges = np.histogram(intervals, bins=np.arange(0.375, 0.865, 0.01))
    center = edges[np.argmax(hist)] + 0.005
    period = np.median(intervals[abs(intervals - center) < 0.045])
    near = abs(delta - period) < 0.05
    beats = beats[np.r_[near, False] | np.r_[False, near]]
    if len(beats) < 8:
        raise ValueError("too few steady kicks to score rhythm")
    indices = np.r_[0, np.cumsum(np.maximum(1, np.round(np.diff(beats) / period)))]
    reference_period = float(np.polyfit(indices, beats, 1)[0])
    nod_indices, _ = find_peaks(
        pitch,
        prominence=math.radians(1.5),
        distance=max(1, int(0.28 / np.median(np.diff(t)))),
    )
    nods = []
    for i in nod_indices:
        curvature = pitch[i - 1] - 2 * pitch[i] + pitch[i + 1]
        offset = (
            0.5 * (pitch[i - 1] - pitch[i + 1]) / curvature
            if abs(curvature) > 1e-12
            else 0
        )
        nods.append(t[i] + np.clip(offset, -0.5, 0.5) * (t[i + 1] - t[i - 1]) / 2)
    nods = np.array(nods)
    nods = nods[(nods > t[0] + 1) & (nods < t[-1] - 1)]
    if not len(nods):
        raise ValueError("no physical nods found")
    # Nearest-event pairing plus coverage penalizes the previous 3–3–2 phrase,
    # which had a reasonable BPM but omitted most individual beat accents.
    errors = np.array([nods[np.argmin(abs(nods - beat))] - beat for beat in beats])
    return {
        "reference_bpm": 60 / reference_period,
        "reference_beats": len(beats),
        "physical_nods": len(nods),
        "beat_coverage_90ms": float(np.mean(abs(errors) <= 0.09)),
        "median_offset_ms": float(np.median(errors) * 1000),
        "median_abs_error_ms": float(np.median(abs(errors)) * 1000),
        "p95_abs_error_ms": float(np.percentile(abs(errors), 95) * 1000),
        "reference_times": beats.tolist(),
        "nod_times": nods.tolist(),
        "errors_ms": (errors * 1000).tolist(),
        "clock": "source monotonic timestamps plus estimated ALSA output latency",
        "estimated_playback_latency_ms": clock["estimated_playback_latency_ms"],
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument(
        "--check",
        action="store_true",
        help="Require at least 85% beat coverage and <=100 ms p95 timing error",
    )
    args = parser.parse_args()
    result = score(args.directory)
    (args.directory / "rhythm.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))
    if args.check and (
        result["beat_coverage_90ms"] < 0.85 or result["p95_abs_error_ms"] > 100
    ):
        raise SystemExit("Physical motion does not reliably accent each kick")


if __name__ == "__main__":
    main()
