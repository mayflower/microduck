#!/usr/bin/env python3
"""Compare the ONNX export with the pinned upstream TensorFlow YAMNet implementation.

Requires tensorflow 2.20.0, tf-keras 2.20.1, numpy and onnxruntime. The reference
folder contains params.py, features.py and yamnet.py at music_yamnet.json's revision.
"""

import argparse
import hashlib
import json
import sys
from pathlib import Path

import numpy as np
import onnxruntime as ort

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("reference_dir", type=Path)
parser.add_argument("weights", type=Path)
parser.add_argument(
    "--model",
    type=Path,
    default=Path(__file__).resolve().parents[1] / "pet-detect/models/music_yamnet.onnx",
)
args = parser.parse_args()
metadata = json.loads(args.model.with_suffix(".json").read_text())
for path, expected in [
    (args.weights, metadata["source_weights_sha256"]),
    (args.model, metadata["onnx_sha256"]),
]:
    if hashlib.sha256(path.read_bytes()).hexdigest() != expected:
        raise SystemExit(f"checksum mismatch: {path}")
sys.path.insert(0, str(args.reference_dir.resolve()))
import params
import yamnet

reference = yamnet.yamnet_frames_model(params.Params())
reference.load_weights(str(args.weights))
options = ort.SessionOptions()
options.intra_op_num_threads = 1
options.inter_op_num_threads = 1
session = ort.InferenceSession(
    str(args.model), options, providers=["CPUExecutionProvider"]
)
i = np.arange(15600, dtype=np.float64)
tone = (
    0.15 * np.sin(2 * np.pi * 440 * i / 16000)
    + 0.08 * np.sin(2 * np.pi * 1234 * i / 16000)
    + 0.2 * (i % 4000 < 16)
)
tone = (tone * 32767).astype(np.int16).astype(np.float32) / 32768
cases = {
    "tone": tone,
    "silence": np.zeros(15600, np.float32),
    "noise": np.random.default_rng(1).normal(0, 0.05, 15600).astype(np.float32),
}
report = []
for name, samples in cases.items():
    expected, _, mel = reference(samples)
    actual = session.run(None, {"log_mel": np.asarray(mel)[:96][None]})[0]
    error = float(np.max(np.abs(actual - np.asarray(expected))))
    report.append({"case": name, "max_score_error": error})
    if error >= 1e-5:
        raise SystemExit(f"{name}: score mismatch {error}")
print(json.dumps(report, indent=2))
