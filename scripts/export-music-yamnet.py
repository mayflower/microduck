#!/usr/bin/env python3
"""Requires Python 3.12, torch 2.9.1, h5py 3.16.0, numpy and onnx. See docs/design/music-dance.md."""

import argparse
import hashlib
import json
from pathlib import Path

import h5py
import numpy as np
import torch
import torch.nn.functional as F
from torch import nn

parser = argparse.ArgumentParser(
    description="Export official YAMNet v1 H5 weights to a log-mel ONNX classifier"
)
parser.add_argument("weights", type=Path)
parser.add_argument("output", type=Path)
args = parser.parse_args()
metadata_path = (
    Path(__file__).resolve().parents[1] / "pet-detect/models/music_yamnet.json"
)
metadata = json.loads(metadata_path.read_text())
if (
    hashlib.sha256(args.weights.read_bytes()).hexdigest()
    != metadata["source_weights_sha256"]
):
    raise SystemExit("source weights do not match the pinned YAMNet v1 checksum")
f = h5py.File(args.weights)


def arr(name):
    group = f[name]
    keys = []
    group.visititems(lambda k, v: keys.append(k) if hasattr(v, "shape") else None)
    return {k.split("/")[-1].split(":")[0]: np.asarray(group[k]) for k in keys}


class Conv(nn.Module):
    def __init__(self, name, ci, co, k, stride, depth=False):
        super().__init__()
        self.k = k
        self.stride = stride
        self.conv = nn.Conv2d(ci, co, k, stride, groups=ci if depth else 1, bias=False)
        self.bn = nn.BatchNorm2d(co, eps=1e-4)
        a = arr(name)
        w = a["depthwise_kernel" if depth else "kernel"]
        w = w.transpose(2, 3, 0, 1) if depth else w.transpose(3, 2, 0, 1)
        self.conv.weight.data.copy_(torch.from_numpy(w.copy()))
        b = arr(name + "/bn")
        self.bn.weight.data.fill_(1)
        self.bn.bias.data.copy_(torch.from_numpy(b["beta"]))
        self.bn.running_mean.copy_(torch.from_numpy(b["moving_mean"]))
        self.bn.running_var.copy_(torch.from_numpy(b["moving_variance"]))

    def forward(self, x):
        h, w = x.shape[-2:]
        ph = max(
            (h + self.stride - 1) // self.stride * self.stride
            - self.stride
            + self.k
            - h,
            0,
        )
        pw = max(
            (w + self.stride - 1) // self.stride * self.stride
            - self.stride
            + self.k
            - w,
            0,
        )
        return F.relu(
            self.bn(self.conv(F.pad(x, (pw // 2, pw - pw // 2, ph // 2, ph - ph // 2))))
        )


class Yamnet(nn.Module):
    def __init__(self):
        super().__init__()
        layers = [Conv("layer1/conv", 1, 32, 3, 2)]
        ci = 32
        for i, (co, s) in enumerate(
            [
                (64, 1),
                (128, 2),
                (128, 1),
                (256, 2),
                (256, 1),
                (512, 2),
                (512, 1),
                (512, 1),
                (512, 1),
                (512, 1),
                (512, 1),
                (1024, 2),
                (1024, 1),
            ],
            2,
        ):
            layers.extend(
                [
                    Conv(f"layer{i}/depthwise_conv", ci, ci, 3, s, True),
                    Conv(f"layer{i}/pointwise_conv", ci, co, 1, 1),
                ]
            )
            ci = co
        self.layers = nn.Sequential(*layers)
        self.linear = nn.Linear(1024, 521)
        a = arr("logits")
        self.linear.weight.data.copy_(torch.from_numpy(a["kernel"].T.copy()))
        self.linear.bias.data.copy_(torch.from_numpy(a["bias"]))

    def forward(self, x):
        return self.linear(self.layers(x.unsqueeze(1)).mean((2, 3))).sigmoid()


m = Yamnet().eval()
torch.onnx.export(
    m,
    (torch.zeros(1, 96, 64),),
    str(args.output),
    input_names=["log_mel"],
    output_names=["scores"],
    opset_version=17,
    dynamo=False,
)
print(args.output, args.output.stat().st_size)
