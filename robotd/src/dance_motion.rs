//! A small beat-driven path, followed from measured contact odometry.
use std::f64::consts::TAU;

pub const MAX_SPEED: f64 = 0.38;
pub const MAX_YAW_RATE: f64 = 0.8;
const AREA_RADIUS: f64 = 0.50;

#[derive(Default)]
pub struct Footwork {
    origin: Option<[f64; 3]>,
    halted: bool,
    path_start: Option<f64>,
}

fn wrap(angle: f64) -> f64 {
    (angle + std::f64::consts::PI).rem_euclid(TAU) - std::f64::consts::PI
}

impl Footwork {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn halted(&self) -> bool {
        self.halted
    }

    pub fn command(&mut self, beat: f64, bpm: f64, pose: [f64; 3]) -> [f64; 3] {
        if !beat.is_finite() || !bpm.is_finite() || pose.iter().any(|v| !v.is_finite()) {
            return [0.0; 3];
        }
        let origin = *self.origin.get_or_insert(pose);
        let [ox, oy, yaw0] = origin;
        let [x, y, yaw] = pose;
        if (x - ox).hypot(y - oy) > AREA_RADIUS {
            self.halted = true;
        }
        if self.halted {
            return [0.0; 3];
        }
        // The head/beat clock can run while acquiring a planted start. Anchor
        // path progress when locomotion begins, without chasing a stale target.
        let start = *self.path_start.get_or_insert(beat.floor());
        let phase = TAU * (beat - start).max(0.0) / 16.0;
        let rate = TAU / 16.0 * (bpm / 60.0);
        let local = [0.20 * phase.sin(), 0.15 * (2.0 * phase).sin()];
        let feed = [0.20 * phase.cos() * rate, 0.30 * (2.0 * phase).cos() * rate];
        let (s, c) = yaw0.sin_cos();
        let target = [
            ox + c * local[0] - s * local[1],
            oy + s * local[0] + c * local[1],
        ];
        let world = [
            c * feed[0] - s * feed[1] + 2.0 * (target[0] - x),
            s * feed[0] + c * feed[1] + 2.0 * (target[1] - y),
        ];
        let (s, c) = yaw.sin_cos();
        let mut velocity = [
            c * world[0] + s * world[1],
            -s * world[0] + c * world[1],
            0.0,
        ];
        let speed = velocity[0].hypot(velocity[1]);
        if speed > MAX_SPEED {
            velocity[0] *= MAX_SPEED / speed;
            velocity[1] *= MAX_SPEED / speed;
        }
        let heading = yaw0 + 0.6 * phase.sin();
        velocity[2] = (0.6 * phase.cos() * rate + 2.0 * wrap(heading - yaw))
            .clamp(-MAX_YAW_RATE, MAX_YAW_RATE);
        velocity
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_cycle_is_curved_closed_and_speed_bounded() {
        let mut f = Footwork::default();
        let mut pose = [0.0; 3];
        let mut points = Vec::new();
        let dt = 0.002;
        for i in 0..4000 {
            let beat = i as f64 * dt * 2.0;
            let v = f.command(beat, 120.0, pose);
            assert!(v[0].hypot(v[1]) <= MAX_SPEED + 1e-10);
            assert!(v[2].abs() <= MAX_YAW_RATE);
            let (s, c) = pose[2].sin_cos();
            pose[0] += (c * v[0] - s * v[1]) * dt;
            pose[1] += (s * v[0] + c * v[1]) * dt;
            pose[2] += v[2] * dt;
            points.push(pose);
        }
        let extent = |axis| {
            points
                .iter()
                .map(|p| p[axis])
                .fold(f64::NEG_INFINITY, f64::max)
                - points.iter().map(|p| p[axis]).fold(f64::INFINITY, f64::min)
        };
        assert!(
            extent(0) > 0.14 && extent(1) > 0.10,
            "a straight-line walk is not the dance path"
        );
        assert!(
            pose[0].hypot(pose[1]) < 0.005,
            "each cycle returns near its origin"
        );
        assert!(pose[2].abs() < 0.02);
    }
    #[test]
    fn measured_error_is_corrected_and_an_escape_latches_stop() {
        let mut f = Footwork::default();
        f.command(0.0, 120.0, [0.0; 3]);
        assert!(f.command(4.0, 120.0, [0.30, 0.0, 0.0])[0] < 0.0);
        assert_eq!(f.command(5.0, 120.0, [0.51, 0.0, 0.0]), [0.0; 3]);
        assert!(f.halted());
        assert_eq!(f.command(6.0, 120.0, [0.0; 3]), [0.0; 3]);
        f.reset();
        assert_ne!(f.command(0.0, 120.0, [0.51, 0.0, 0.0]), [0.0; 3]);
    }
    #[test]
    fn the_pattern_rotates_with_its_anchor_and_heading_wrap_is_continuous() {
        let mut a = Footwork::default();
        let mut b = Footwork::default();
        let va = a.command(2.0, 120.0, [0.0, 0.0, 0.0]);
        let vb = b.command(2.0, 120.0, [3.0, -2.0, 1.7]);
        for i in 0..3 {
            assert!((va[i] - vb[i]).abs() < 1e-12);
        }
        let wrapped = b.command(2.0, 120.0, [3.0, -2.0, 1.7 + TAU]);
        for i in 0..3 {
            assert!((vb[i] - wrapped[i]).abs() < 1e-12);
        }
    }
    #[test]
    fn invalid_odometry_never_commands_motion() {
        assert_eq!(
            Footwork::default().command(0.0, 120.0, [f64::NAN, 0.0, 0.0]),
            [0.0; 3]
        );
    }
}
