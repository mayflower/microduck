#!/usr/bin/env python3
"""Measure physical foot clearance and path curvature from saved MuJoCo frames."""

import argparse
import json
from itertools import pairwise
from pathlib import Path

import mujoco
import numpy as np
from mjlab_microduck.sim.body_server import DEFAULT_SCENE, Body, World

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("directory", type=Path)
parser.add_argument("--check", action="store_true")
parser.add_argument("--check-rhythm", action="store_true")
args = parser.parse_args()
frames = json.loads((args.directory / "body-states.json").read_text())
world = World(DEFAULT_SCENE, 1)
body = Body(world, 0)
model, data = world.model, world.data
sides = ["left", "right"]
geoms = [model.geom(side + "_foot_collision").id for side in sides]
vertices = []
for geom in geoms:
    mesh = model.geom_dataid[geom]
    start, count = model.mesh_vertadr[mesh], model.mesh_vertnum[mesh]
    vertices.append(model.mesh_vert[start : start + count].copy())
rows = []
for frame in frames:
    data.qpos[body.trunk : body.trunk + 3] = frame["trunk"]
    data.qpos[body.trunk + 3 : body.trunk + 7] = frame["imu"]["quat"]
    data.qpos[body.qpos_adr] = np.asarray(frame["positions"])[body.to_wire]
    mujoco.mj_forward(model, data)
    # Lowest collision-mesh vertex, not ankle height: a planted, tilted foot must
    # not count as airborne. This scene's floor is the world z=0 plane.
    clearance = [
        float(
            (verts @ data.geom_xmat[g].reshape(3, 3).T + data.geom_xpos[g])[:, 2].min()
        )
        for g, verts in zip(geoms, vertices)
    ]
    rows.append((frame["t_ns"] / 1e9, clearance))
report = {}
for axis, side in enumerate(sides):
    height = np.array([row[1][axis] for row in rows])
    since = None
    landings = []
    previous = rows[0]
    for timestamp, clearance in rows:
        if clearance[axis] > 0.004 and since is None:
            since = timestamp
        if clearance[axis] <= 0.0005 and since is not None:
            if timestamp - since > 0.03:
                t0, z0 = previous[0], previous[1][axis]
                fraction = np.clip(
                    (z0 - 0.0005) / max(z0 - clearance[axis], 1e-12), 0, 1
                )
                landings.append(float(t0 + fraction * (timestamp - t0)))
            since = None
        previous = (timestamp, clearance)
    report[side] = {
        "max_clearance_mm": float(height.max() * 1000),
        "air_fraction": float(np.mean(height > 0.004)),
        "landings": landings,
    }
xy = np.array([f["trunk"][:2] for f in frames])
xy -= xy[0].copy()
variance, directions = np.linalg.eigh(np.cov(xy.T))
perpendicular = (xy - xy.mean(axis=0)) @ directions[:, 0]
report["path"] = {
    "span_xy_m": np.ptp(xy, axis=0).tolist(),
    "max_radius_m": float(np.linalg.norm(xy, axis=1).max()),
    "minor_axis_rms_m": float(np.sqrt(variance[0])),
    "perpendicular_p95_m": float(np.percentile(abs(perpendicular), 95)),
    "minor_variance_fraction": float(variance[0] / max(variance.sum(), 1e-12)),
}
if args.check_rhythm:
    reference = json.loads((args.directory / "rhythm.json").read_text())
    clock = json.loads((args.directory / "observation-audio-clock.json").read_text())
    beats = np.array(reference["reference_times"])
    period = 60 / reference["reference_bpm"]
    # Quarter notes only: never adapt the reference to an observed faster gait.
    steps = 1
    grid = []
    for first, second in pairwise(beats):
        if 0.75 * period < second - first < 1.25 * period:
            grid.extend(first + np.arange(steps) * (second - first) / steps)
    grid = np.array(grid)
    events = sorted(
        (
            timestamp
            - clock["source_start_monotonic_ns"] / 1e9
            - clock["estimated_playback_latency_ms"] / 1000,
            side,
        )
        for side in sides
        for timestamp in report[side]["landings"]
    )
    tolerance = min(0.070, period / steps * 0.25)
    # A valid early/late contact at either boundary still belongs to that beat.
    events = [
        (t, side)
        for t, side in events
        if grid[0] - tolerance <= t <= grid[-1] + tolerance
    ]
    actual = np.array([event[0] for event in events])
    distances = abs(grid[:, None] - actual[None, :])
    # Allow unmatched beats/contacts. Forcing all events into pairs can shift
    # the entire assignment after one missed beat and one extra balance step.
    from scipy.optimize import linear_sum_assignment

    size = len(grid) + len(actual)
    cost = np.zeros((size, size))
    cost[: len(grid), : len(actual)] = np.where(distances <= tolerance, distances, 1e6)
    cost[: len(grid), len(actual) :] = tolerance
    cost[len(grid) :, : len(actual)] = tolerance
    ri, ci = linear_sum_assignment(cost)
    pairs = (ri < len(grid)) & (ci < len(actual))
    count = int(np.sum(pairs))
    timing_error = (
        max(
            np.percentile(distances.min(axis=1), 95),
            np.percentile(distances.min(axis=0), 95),
        )
        * 1000
        if len(actual)
        else None
    )
    alternation = (
        np.mean([a[1] != b[1] for a, b in pairwise(events)]) if len(events) > 1 else 0.0
    )
    report["rhythm"] = {
        "steps_per_beat": steps,
        "expected_steps": len(grid),
        "actual_steps": len(actual),
        "median_interval_beats": float(np.median(np.diff(actual)) / period)
        if len(actual) > 1
        else None,
        "coverage": float(count / len(grid)),
        "precision": float(count / max(1, len(actual))),
        "p95_error_ms": float(timing_error) if timing_error is not None else None,
        "alternating_fraction": float(alternation),
        "tolerance_ms": tolerance * 1000,
        "estimated_playback_latency_ms": clock["estimated_playback_latency_ms"],
    }
(args.directory / "footwork.json").write_text(json.dumps(report, indent=2) + "\n")
summary = {**report}
for side in sides:
    summary[side] = {k: v for k, v in report[side].items() if k != "landings"}
    summary[side]["landing_count"] = len(report[side]["landings"])
print("Physical footwork:", json.dumps(summary), flush=True)
if args.check:
    for side in sides:
        assert len(report[side]["landings"]) >= 4, f"{side} foot stays planted"
        assert report[side]["max_clearance_mm"] >= 6, f"{side} foot barely lifts"
    assert report["path"]["minor_axis_rms_m"] >= 0.010, "path is nearly a straight line"
    # Absolute deviation distinguishes an elongated curved path from a straight
    # diagonal walk; a variance ratio alone also penalizes legitimate long arcs.
    assert report["path"]["perpendicular_p95_m"] >= 0.020, (
        "path is nearly a straight line"
    )
    assert report["path"]["max_radius_m"] < 0.55, "dance leaves its local area"

if args.check_rhythm:
    rhythm = report["rhythm"]
    assert rhythm["coverage"] >= 0.90, "feet miss quarter-note beats"
    assert rhythm["precision"] >= 0.90, "extra offbeat footsteps"
    assert rhythm["p95_error_ms"] is not None and rhythm["p95_error_ms"] <= 70, (
        "footfalls drift off the beat"
    )
    assert rhythm["alternating_fraction"] >= 0.90, "feet do not alternate"

if args.check_rhythm:
    interval = report["rhythm"]["median_interval_beats"]
    assert interval is not None and 0.85 <= interval <= 1.15, (
        "footfall cadence is not quarter notes"
    )
