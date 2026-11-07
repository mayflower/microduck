#!/usr/bin/env python3
"""Extract a cyclic leg forcing profile from a local VelStand simulation.

Run with the RL Python environment. No microphone, robot, training or network is used.
The profile omits the mean pose; live balance remains the policy's responsibility.
"""

import argparse
import hashlib
import json
from pathlib import Path

import mujoco
import numpy as np
import onnxruntime as ort
from mjlab_microduck.sim.body_server import (
    DEFAULT_SCENE,
    HOME_POSE,
    HOME_TRUNK_Z,
    Body,
    World,
)

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--policy", type=Path, required=True)
parser.add_argument("--output", type=Path, required=True)
args = parser.parse_args()
options = ort.SessionOptions()
options.intra_op_num_threads = options.inter_op_num_threads = 1
net = ort.InferenceSession(
    str(args.policy), options, providers=["CPUExecutionProvider"]
)
input_name = net.get_inputs()[0].name
world = World(DEFAULT_SCENE, 1)
body = Body(world, 0)
world.bodies.append(body)
body.place(None, HOME_TRUNK_Z, 0)
body.set_torque(True)
model, data = world.model, world.data
home = np.array(HOME_POSE)
wire = np.array([i for i in range(15) if i != 9])
previous = np.zeros(14)
geom = model.geom("left_foot_collision").id
mesh = model.geom_dataid[geom]
start, count = model.mesh_vertadr[mesh], model.mesh_vertnum[mesh]
vertices = model.mesh_vert[start : start + count].copy()
clearances, targets, states = [], [], []
for tick in range(600):
    sensor = body.sensors()
    command = [0.35 if tick > 100 else 0, 0, 0]
    observation = np.r_[
        sensor["imu"]["gyro"],
        sensor["imu"]["gravity"],
        np.array(sensor["positions"])[wire] - home[wire],
        np.array(sensor["velocities"])[wire],
        previous,
        command,
        np.zeros(10),
    ].astype(np.float32)
    action = net.run(None, {input_name: observation[None]})[0][0]
    target = home.copy()
    target[wire] = home[wire] + 0.9 * action
    body.set_targets(target)
    world.step(4)
    previous = action
    sensor = body.sensors()
    assert sensor["trunk_z"] > 0.10 and sensor["imu"]["gravity"][2] < -0.96, (
        "reference lost balance"
    )
    targets.append(target)
    states.append(sensor)
# Reconstruct after the rollout so querying collision geometry cannot change the
# simulator's integration/warm-start state while the reference is being recorded.
for sensor in states:
    data.qpos[body.trunk : body.trunk + 3] = sensor["trunk"]
    data.qpos[body.trunk + 3 : body.trunk + 7] = sensor["imu"]["quat"]
    data.qpos[body.qpos_adr] = np.asarray(sensor["positions"])[body.to_wire]
    mujoco.mj_forward(model, data)
    clearances.append(
        float(
            (vertices @ data.geom_xmat[geom].reshape(3, 3).T + data.geom_xpos[geom])[
                :, 2
            ].min()
        )
    )
height = np.array(clearances)
time = np.arange(len(height)) * 0.02
landings = np.where((height[1:] < 0.002) & (height[:-1] >= 0.002))[0] + 1
landings = landings[time[landings] > 4]
assert len(landings) >= 12 and np.all(
    (np.diff(time[landings]) > 0.30) & (np.diff(time[landings]) < 0.60)
), "no regular reference steps"
phase = np.interp(time, time[landings], np.arange(len(landings)))
keep = (time >= time[landings[0]]) & (time < time[landings[-1]])
basis = np.stack(
    [np.ones_like(phase)]
    + [f(2 * np.pi * k * phase) for k in range(1, 5) for f in [np.cos, np.sin]],
    axis=-1,
)
coefficients = np.linalg.lstsq(basis[keep], np.array(targets)[keep], rcond=None)[0]
profile = {
    "source": "VelStand, 0.35 m/s forward, 50 Hz controller / 200 Hz MuJoCo, harmonic fit over settled left-contact cycles",
    "policy_sha256": hashlib.sha256(args.policy.read_bytes()).hexdigest(),
    "joints": [1, 3, 11, 13],
    "coefficients": coefficients[1:, [1, 3, 11, 13]].tolist(),
}
args.output.write_text(json.dumps(profile, indent=2) + "\n")
print(f"Saved {len(landings) - 1} settled gait cycles to {args.output}")
