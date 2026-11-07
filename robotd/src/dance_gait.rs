//! One alternating step per quarter-note beat, with double support between steps.
use std::f64::consts::TAU;
use std::sync::LazyLock;
const CLOCK_OFFSET: f64 = -0.04;

#[derive(serde::Deserialize)]
struct Cycle {
    policy_sha256: String,
    joints: [usize; 4],
    coefficients: [[f64; 4]; 8],
}
static CYCLE: LazyLock<Cycle> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../assets/dance-cycle.json")).expect("embedded dance cycle")
});
static POLICY_DIGEST: LazyLock<[u8; 32]> = LazyLock::new(|| {
    assert_eq!(CYCLE.policy_sha256.len(), 64, "embedded policy digest");
    std::array::from_fn(|i| {
        u8::from_str_radix(&CYCLE.policy_sha256[i * 2..i * 2 + 2], 16)
            .expect("embedded policy digest")
    })
});

pub fn initialize() {
    LazyLock::force(&CYCLE);
    LazyLock::force(&POLICY_DIGEST);
}

pub fn supports_policy(digest: &[u8; 32]) -> bool {
    digest == &*POLICY_DIGEST
}

fn wave(phase: f64) -> [f64; 4] {
    std::array::from_fn(|column| {
        (0..4)
            .map(|harmonic| {
                let angle = TAU * (harmonic + 1) as f64 * phase;
                CYCLE.coefficients[2 * harmonic][column] * angle.cos()
                    + CYCLE.coefficients[2 * harmonic + 1][column] * angle.sin()
            })
            .sum()
    })
}

/// A short swing has the original gait's duration; the remainder of each beat
/// is planted standing. Slowing the whole gait let its natural steps fill gaps.
fn swing(beat: f64, bpm: f64) -> Option<(f64, f64)> {
    if !beat.is_finite() || !bpm.is_finite() || !(70.0..=160.0).contains(&bpm) {
        return None;
    }
    let clock = beat + CLOCK_OFFSET;
    let width = (0.195 * bpm / 60.0).min(0.8);
    let progress = ((clock.rem_euclid(1.0) - (1.0 - width)) / width).clamp(0.0, 1.0);
    Some((clock.floor() * 0.5 + 0.4, progress))
}

pub fn safe_to_correct(beat: f64, bpm: f64) -> bool {
    let phase = (beat + CLOCK_OFFSET).rem_euclid(1.0);
    let width = (0.195 * bpm / 60.0).min(0.8);
    phase > 0.08 && phase < 1.0 - width - 0.08
}

pub fn drive_gate(beat: f64, bpm: f64) -> f64 {
    swing(beat, bpm).map_or(0.0, |(_, u)| if u > 0.0 { 0.2 } else { 0.0 })
}

pub fn apply(targets: &mut [f64; 15], beat: f64, bpm: f64, amplitude: f64) {
    if !amplitude.is_finite() {
        return;
    }
    let Some((start, u)) = swing(beat, bpm) else {
        return;
    };
    let current = wave(start + u * 0.5);
    let from = wave(start);
    let to = wave(start + 0.5);
    for (column, joint) in CYCLE.joints.iter().enumerate() {
        // Remove the endpoint line as well as the DC pose: both ends now join
        // the ordinary standing controller at zero offset between steps.
        let offset = current[column] - ((1.0 - u) * from[column] + u * to[column]);
        let strength = if *joint == 1 || *joint == 11 {
            2.25
        } else {
            1.1
        };
        targets[*joint] += amplitude.clamp(0.0, 1.0) * strength * offset;
    }
}

/// Correct the swing clock from kinematic landings, only while planted.
#[derive(Default)]
pub struct ContactClock {
    shift: f64,
    pending: f64,
    airborne: [bool; 2],
    previous: [f64; 2],
    previous_beat: Option<f64>,
    last_contact: Option<f64>,
    good: u8,
    locked: bool,
    head_samples: [(f64, f64); 3],
    head_samples_seen: u8,
    head_minimum: Option<f64>,
    head_at: Option<f64>,
    head_count: u8,
    head_phase: Option<f64>,
    previous_error: Option<f64>,
}
impl ContactClock {
    pub fn shift(&self) -> f64 {
        self.shift
    }
    pub fn locked(&self) -> bool {
        self.locked
    }
    /// Measure actuator phase from the actual once-per-beat head pulse. The
    /// independent simulation scorer still checks head AND feet against music.
    pub fn update_head(&mut self, beat: f64, pitch: f64) {
        if !beat.is_finite() || !pitch.is_finite() {
            return;
        }
        self.head_samples.rotate_left(1);
        self.head_samples[2] = (beat, pitch);
        self.head_samples_seen = self.head_samples_seen.saturating_add(1);
        self.head_minimum = Some(self.head_minimum.map_or(pitch, |p| p.min(pitch)));
        if self.head_samples_seen < 3 {
            return;
        }
        let [(b0, p0), (b1, p1), (b2, p2)] = self.head_samples;
        if p1 > p0
            && p1 >= p2
            && p1 - self.head_minimum.unwrap_or(p1) > 0.03
            && self.head_at.is_none_or(|old| b1 - old > 0.6)
        {
            let curvature = p0 - 2.0 * p1 + p2;
            let offset = if curvature.abs() > 1e-12 {
                0.5 * (p0 - p2) / curvature
            } else {
                0.0
            };
            let at = b1 + offset.clamp(-0.5, 0.5) * (b2 - b0) / 2.0;
            let phase = at.rem_euclid(1.0);
            self.head_phase = Some(self.head_phase.map_or(phase, |old| {
                old + 0.7 * ((phase - old + 0.5).rem_euclid(1.0) - 0.5)
            }));
            self.head_at = Some(at);
            self.head_count = self.head_count.saturating_add(1);
            self.head_minimum = Some(pitch);
        }
    }

    pub fn update(&mut self, beat: f64, bpm: f64, clearance: [f64; 2], planted: bool) {
        if !beat.is_finite()
            || !bpm.is_finite()
            || bpm <= 0.0
            || clearance.iter().any(|z| !z.is_finite())
        {
            return;
        }
        for (foot, &z) in clearance.iter().enumerate() {
            if z > 0.004 {
                self.airborne[foot] = true;
            }
            if z <= 0.001 && self.airborne[foot] {
                self.airborne[foot] = false;
                let fraction = ((self.previous[foot] - 0.001)
                    / (self.previous[foot] - z).max(1e-9))
                .clamp(0.0, 1.0);
                let at = self
                    .previous_beat
                    .map_or(beat, |old| old + (beat - old) * fraction);
                let interval = self.last_contact.map(|old| at - old);
                if interval.is_none_or(|dt| dt > 0.6) {
                    let target = self.head_phase.unwrap_or(0.25) + 0.015 * bpm / 60.0;
                    let error = (at - target + 0.5).rem_euclid(1.0) - 0.5;
                    if interval.is_none_or(|dt| (dt - dt.round()).abs() < 0.25) {
                        // Average opposite-foot errors instead of chasing their
                        // small alternating response differences on every beat.
                        let filtered = self.previous_error.map_or(error, |old| (old + error) * 0.5);
                        self.previous_error = Some(error);
                        let (gain, limit) = if self.locked {
                            (0.35, 0.03)
                        } else {
                            (0.5, 0.06)
                        };
                        self.pending = if self.locked && filtered.abs() * 60.0 / bpm < 0.010 {
                            0.0
                        } else {
                            (filtered * gain).clamp(-limit, limit)
                        };
                    }
                    let on_time = error.abs() * 60.0 / bpm < 0.03;
                    let quarter = interval.is_none_or(|dt| (0.75..=1.25).contains(&dt));
                    self.good = if on_time && quarter {
                        self.good.saturating_add(1)
                    } else {
                        0
                    };
                    self.locked |= self.good >= 3 && self.head_count >= 2;
                    tracing::debug!(
                        foot,
                        error_ms = error * 60.0 / bpm * 1000.0,
                        shift = self.shift,
                        locked = self.locked,
                        "dance foot contact"
                    );
                    self.last_contact = Some(at);
                } else {
                    self.good = 0;
                }
            }
        }
        if planted {
            self.shift = (self.shift + self.pending).clamp(-0.4, 0.4);
            self.pending = 0.0;
        }
        self.previous = clearance;
        self.previous_beat = Some(beat);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn footwork_is_bound_to_the_calibrated_model_bytes() {
        // Independently pin the VelStand v8 used for the recorded physical trials;
        // merely editing a profile's metadata must not authorize another model.
        let digest = [
            0x2f, 0xc7, 0xf8, 0xee, 0x6a, 0xcd, 0x7d, 0x0d, 0x3f, 0x66, 0xa9, 0x99, 0x27, 0xb5,
            0xd5, 0xa5, 0x65, 0x89, 0xae, 0xed, 0x02, 0xec, 0x71, 0x4d, 0xe3, 0x29, 0xb2, 0x52,
            0xd5, 0xcc, 0xf4, 0x92,
        ];
        assert!(supports_policy(&digest));
        let mut changed = digest;
        changed[0] ^= 1;
        assert!(!supports_policy(&changed));
    }
    #[test]
    fn one_swing_window_per_beat_and_each_leg_repeats_every_two_beats() {
        for bpm in [70.0, 90.0, 120.0, 150.0] {
            let mut starts = 0;
            let mut active = false;
            for i in 0..4000 {
                let beat = 0.25 + i as f64 / 1000.0;
                let on = drive_gate(beat, bpm) > 0.0;
                if on && !active {
                    starts += 1;
                }
                active = on;
                let mut a = [0.0; 15];
                let mut b = a;
                apply(&mut a, beat, bpm, 1.0);
                apply(&mut b, beat + 2.0, bpm, 1.0);
                for j in 0..15 {
                    assert!((a[j] - b[j]).abs() < 1e-11);
                }
                if !on {
                    assert!(a.iter().all(|v| v.abs() < 1e-12));
                }
            }
            assert_eq!(starts, 4, "one landing opportunity per quarter note");
        }
    }
    #[test]
    fn no_amplitude_or_invalid_clock_cannot_move_legs() {
        for (beat, bpm, amp) in [
            (1.0, 120.0, 0.0),
            (f64::NAN, 120.0, 1.0),
            (1.0, f64::NAN, 1.0),
        ] {
            let mut q = [0.1; 15];
            apply(&mut q, beat, bpm, amp);
            assert_eq!(q, [0.1; 15]);
        }
    }
    #[test]
    fn contact_feedback_corrects_late_landings_without_changing_the_step_count() {
        let mut clock = ContactClock {
            head_phase: Some(0.3),
            head_count: 2,
            ..Default::default()
        };
        for step in 0..14 {
            let at = step as f64 + 0.48 - clock.shift();
            clock.update(at - 0.1, 120.0, [0.008, 0.0], true);
            clock.update(at, 120.0, [0.001, 0.0], true);
        }
        assert!(clock.locked());
        assert!((clock.shift() - 0.15).abs() < 0.03);
        let before = clock.shift();
        clock.update(f64::NAN, 120.0, [0.0; 2], true);
        assert_eq!(clock.shift(), before);
    }
    #[test]
    fn physical_head_peaks_supply_phase_without_assuming_motor_latency() {
        let mut clock = ContactClock::default();
        for i in 0..300 {
            let beat = i as f64 * 0.04;
            clock.update_head(beat, 0.2 * (TAU * (beat - 0.2)).cos());
        }
        assert!(clock.head_count >= 8);
        assert!((clock.head_phase.unwrap() - 0.2).abs() < 0.01);
    }
}
