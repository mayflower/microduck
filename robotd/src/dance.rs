//! Autonomous command overlay; never owns I/O or writes shared manual intents.
use crate::dance_motion::{Footwork, MAX_SPEED};
use duck_control::obs::{BodyPose, Command};
use pet_detect::music::MusicFrame;
use std::time::{Duration, Instant};
const BODY_ROLL_LIMIT: f64 = 7.0;
const BODY_PITCH_LIMIT: f64 = 4.0;
const HEAD_PITCH_LIMIT: f64 = 12.0;
const HEAD_YAW_LIMIT: f64 = 16.0;

/// Choreographic angles in degrees, converted once at the command boundary.
#[derive(Clone, Copy)]
struct Pose {
    roll: f64,
    pitch: f64,
    nod: f64,
    look: f64,
}

const PHRASE: [Pose; 8] = [
    Pose {
        roll: -BODY_ROLL_LIMIT,
        pitch: 0.0,
        nod: -6.0,
        look: HEAD_YAW_LIMIT,
    },
    Pose {
        roll: BODY_ROLL_LIMIT,
        pitch: 1.0,
        nod: 10.0,
        look: -HEAD_YAW_LIMIT,
    },
    Pose {
        roll: -BODY_ROLL_LIMIT,
        pitch: -1.0,
        nod: -6.0,
        look: HEAD_YAW_LIMIT,
    },
    Pose {
        roll: BODY_ROLL_LIMIT,
        pitch: 1.0,
        nod: 10.0,
        look: -HEAD_YAW_LIMIT,
    },
    Pose {
        roll: 0.0,
        pitch: -2.0,
        nod: -10.0,
        look: 0.0,
    },
    Pose {
        roll: 0.0,
        pitch: 2.0,
        nod: 10.0,
        look: 0.0,
    },
    Pose {
        roll: 0.0,
        pitch: BODY_PITCH_LIMIT,
        nod: HEAD_PITCH_LIMIT,
        look: 0.0,
    },
    Pose {
        roll: 0.0,
        pitch: 1.0,
        nod: -4.0,
        look: 12.0,
    },
];

/// Each eight-beat phrase mirrors the last. These are counted beats, not a claim
/// to detect the song's downbeat. Quintic transitions land with zero speed and
/// acceleration; the short hold makes the accents distinct from a sine wave.
fn choreography(beat: f64) -> Command {
    let index = beat.floor() as usize;
    let pose = |i: usize| {
        let mut p = PHRASE[i % 8];
        let mirror = if (i / 8).is_multiple_of(2) { 1.0 } else { -1.0 };
        p.roll *= mirror;
        p.look *= mirror;
        p
    };
    let from = pose(index);
    let to = pose(index + 1);
    let hold = if index % 8 == 7 { 0.55 } else { 0.12 };
    let t = ((beat.fract() - hold) / (1.0 - hold)).clamp(0.0, 1.0);
    let ease = (t * t * t * (t * (6.0 * t - 15.0) + 10.0)).clamp(0.0, 1.0);
    let lerp = |a: f64, b: f64| a + (b - a) * ease;
    Command {
        twist: [0.0; 3],
        head: [
            0.0,
            lerp(from.nod, to.nod).to_radians(),
            lerp(from.look, to.look).to_radians(),
            0.0,
        ],
        body: BodyPose {
            z: 0.0,
            roll: lerp(from.roll, to.roll).to_radians(),
            pitch: lerp(from.pitch, to.pitch).to_radians(),
        },
    }
}

/// A physical accent on EVERY beat, independent of the slower pose sequence.
/// The second harmonic makes the downstroke distinct without adding a second
/// pulse. The peak remains at phase zero for all tempos and phrases.
fn beat_pulse(beat: f64) -> f64 {
    let angle = std::f64::consts::TAU * beat;
    (angle.cos() + 0.15 * (2.0 * angle).cos()) / 1.15
}

fn rhythmic_command(beat: f64, bpm: f64) -> Command {
    let mut c = choreography(beat);
    // Keep the per-beat pulse dominant; alternating poses alone produced a 3–3–2
    // nod pattern. At high tempo reduce excursion to preserve the accent's timing
    // inside the velocity envelope instead of clipping and landing late.
    let speed_scale = (120.0 / bpm).min(1.0);
    let pulse = beat_pulse(beat);
    c.head[1] = 0.25 * c.head[1] + (9.0 * speed_scale * pulse).to_radians();
    c.body.pitch = 0.5 * c.body.pitch + (1.8 * speed_scale * pulse).to_radians();
    c
}

fn slew(value: &mut f64, target: f64, speed: f64, dt: f64) {
    *value += (target - *value).clamp(-speed * dt, speed * dt);
}

#[derive(Default)]
pub struct Dance {
    phase: f64,
    last_bpm: f64,
    amplitude: f64,
    candidate: Option<Instant>,
    last: Option<Instant>,
    phrase_start: Option<f64>,
    output: Command,
    footwork: Option<Footwork>,
    position: [f64; 3],
    gait_ready: bool,
    contact_clock: crate::dance_gait::ContactClock,
    clearance: [f64; 2],
    measured_head_pitch: f64,
}
impl Dance {
    pub fn new(footwork: bool) -> Self {
        if footwork {
            crate::dance_gait::initialize();
        }
        Self {
            footwork: footwork.then(Footwork::default),
            ..Self::default()
        }
    }
    pub fn update_odometry(&mut self, position: [f64; 3]) -> bool {
        self.position = position;
        position.iter().all(|v| v.is_finite())
    }
    pub fn update_contacts(&mut self, clearance: [f64; 2], head_pitch: f64) {
        self.clearance = clearance;
        self.measured_head_pitch = head_pitch;
    }
    fn leg_beat(&self) -> f64 {
        self.phrase_start
            .map_or(0.0, |start| (self.phase - start).max(0.0))
            + self.contact_clock.shift()
    }

    pub fn apply_footwork(&self, targets: &mut [f64; 15]) {
        if self.walking()
            && let Some(f) = &self.footwork
            && !f.halted()
            && let Some(start) = self.phrase_start
        {
            // Replace this joint's policy target, never double-add the head
            // command. Walking balance otherwise sometimes cancels fast nods.
            targets[6] = duck_control::model::DEFAULT_POSITION[6] + self.output.head[1];
            if self.gait_ready {
                crate::dance_gait::apply(
                    targets,
                    (self.phase - start).max(0.0) + self.contact_clock.shift(),
                    self.last_bpm,
                    self.amplitude,
                );
            }
        }
    }

    pub fn planted(&self) -> bool {
        self.walking()
            && (!self.gait_ready
                || self.phrase_start.is_some_and(|start| {
                    crate::dance_gait::drive_gate(
                        (self.phase - start).max(0.0) + self.contact_clock.shift(),
                        self.last_bpm,
                    ) == 0.0
                }))
    }

    pub fn walking(&self) -> bool {
        self.footwork.is_some() && self.amplitude > 0.0
    }

    pub fn engaged(&self) -> bool {
        self.amplitude > 0.0 || self.candidate.is_some()
    }
    pub fn tick(
        &mut self,
        now: Instant,
        frame: Option<&MusicFrame>,
        enabled: bool,
        eligible: bool,
        manual_age: Duration,
    ) -> (Command, duck_ipc_proto::DanceStatus) {
        let dt = self.last.replace(now).map_or(0.0, |t| {
            now.saturating_duration_since(t).as_secs_f64().min(0.1)
        });
        let audio_fresh =
            frame.is_some_and(|f| now.saturating_duration_since(f.at) < Duration::from_millis(500));
        let frame = frame.filter(|f| {
            now.saturating_duration_since(f.at) < Duration::from_millis(500)
                && f.beat.is_finite()
                && f.locked
                && f.music
                && f.bpm.is_finite()
                && (70.0..=160.0).contains(&f.bpm)
        });
        let reason = if !enabled {
            Some("disabled")
        } else if !eligible {
            Some("robot_not_ready")
        } else if manual_age < Duration::from_secs(10) {
            Some("manual_control")
        } else if !audio_fresh {
            Some("audio_unavailable")
        } else if frame.is_none() {
            Some("no_music_or_beat")
        } else {
            None
        };
        if reason.is_some() {
            self.candidate = None;
        } else if self.candidate.is_none() {
            self.candidate = Some(now);
        }
        let dancing = self
            .candidate
            .is_some_and(|at| now.duration_since(at) >= Duration::from_secs(1));
        // Safety and manual ownership revoke the overlay immediately. Music loss alone fades.
        if !audio_fresh || !eligible || !enabled || manual_age < Duration::from_secs(10) {
            self.amplitude = 0.0;
            self.phrase_start = None;
            self.output = Command::default();
        }
        self.amplitude = (self.amplitude + if dancing { dt } else { -dt }).clamp(0.0, 1.0);
        if let Some(f) = frame {
            self.last_bpm = f.bpm;
            self.phase += dt * f.bpm / 60.0;
            let target =
                (f.beat + now.duration_since(f.at).as_secs_f64() * f.bpm / 60.0).rem_euclid(1.0);
            let correction = (target - self.phase.rem_euclid(1.0) + 0.5).rem_euclid(1.0) - 0.5;
            self.phase += correction * dt.min(0.1) * 2.0;
        } else if self.amplitude > 0.0 {
            // Finish the fade at the last known tempo instead of freezing a
            // partial swing (or a head nod) until its amplitude reaches zero.
            self.phase += dt * self.last_bpm / 60.0;
        }
        if dancing && self.phrase_start.is_none() {
            self.phrase_start = Some(self.phase.floor());
        }
        let beat = self
            .phrase_start
            .map_or(0.0, |start| (self.phase - start).max(0.0));
        if self.amplitude == 0.0 {
            self.gait_ready = false;
            self.contact_clock = crate::dance_gait::ContactClock::default();
            if let Some(footwork) = self.footwork.as_mut() {
                footwork.reset();
            }
        }
        // Finish the head/body ramp in planted standing, then begin a complete
        // swing. Starting halfway through a weak ramped swing added stray steps.
        if self.footwork.is_some()
            && self.amplitude >= 1.0
            && crate::dance_gait::drive_gate(beat, self.last_bpm) == 0.0
        {
            self.gait_ready = true;
        }
        if self.gait_ready && self.amplitude >= 1.0 {
            self.contact_clock
                .update_head(beat, self.measured_head_pitch);
            let planted = crate::dance_gait::safe_to_correct(self.leg_beat(), self.last_bpm);
            self.contact_clock
                .update(beat, self.last_bpm, self.clearance, planted);
        }
        let mut target = rhythmic_command(beat, self.last_bpm.max(70.0));
        if let Some(footwork) = self.footwork.as_mut() {
            target.body = BodyPose::default();
            // Walking introduces head-tracking disturbance. Remove the slower
            // pitch phrase so it cannot cancel a fast per-beat nod; keep gaze.
            let angle = std::f64::consts::TAU * beat;
            target.head[1] =
                (10.5 * (120.0 / self.last_bpm.max(70.0)).min(1.0).powi(3) * angle.cos())
                    .to_radians();
            // Slow gaze sweeps leave balance headroom for one deliberate step
            // per beat; rapid lateral head flips could provoke extra steps.
            target.head[2] = HEAD_YAW_LIMIT.to_radians() * (angle / 8.0).sin();
            if self.amplitude > 0.0 && self.gait_ready {
                target.twist = footwork.command(beat, self.last_bpm, self.position);
                let gate =
                    crate::dance_gait::drive_gate(beat + self.contact_clock.shift(), self.last_bpm);
                for value in &mut target.twist[..2] {
                    *value *= gate;
                }
                let turn_strength = (1.0 - (self.last_bpm - 90.0) / 60.0).clamp(0.2, 1.0);
                target.twist[2] *= if gate > 0.0 { turn_strength } else { 0.0 };
            }
        }
        for v in &mut target.twist {
            *v *= self.amplitude;
        }
        for v in &mut target.head {
            *v *= self.amplitude;
        }
        target.body.roll *= self.amplitude;
        target.body.pitch *= self.amplitude;
        // Even phase corrections and the mirrored phrase boundary cannot ask the
        // servos for a jump. Safety/manual revocation above bypasses this limiter.
        for axis in 0..3 {
            slew(
                &mut self.output.twist[axis],
                target.twist[axis],
                if axis == 2 {
                    if self.footwork.is_some() { 4.0 } else { 1.2 }
                } else {
                    0.9
                },
                dt,
            );
        }
        // The planted phase must be exact zero; a lingering velocity tail can
        // restart the walking policy and add an unwanted eighth-note step.
        if self.footwork.is_some()
            && crate::dance_gait::drive_gate(beat + self.contact_clock.shift(), self.last_bpm)
                == 0.0
        {
            self.output.twist = [0.0; 3];
        }
        // Independent axis slew can briefly leave the circular speed envelope.
        let speed = self.output.twist[0].hypot(self.output.twist[1]);
        if speed > MAX_SPEED {
            self.output.twist[0] *= MAX_SPEED / speed;
            self.output.twist[1] *= MAX_SPEED / speed;
        }
        for (value, target) in self.output.head.iter_mut().zip(target.head) {
            slew(value, target, 140f64.to_radians(), dt);
        }
        slew(
            &mut self.output.body.roll,
            target.body.roll,
            70f64.to_radians(),
            dt,
        );
        slew(
            &mut self.output.body.pitch,
            target.body.pitch,
            50f64.to_radians(),
            dt,
        );
        if self.amplitude == 0.0 {
            self.output = Command::default();
            self.phrase_start = None;
        }
        let command = self.output;
        (
            command,
            duck_ipc_proto::DanceStatus {
                enabled,
                state: if self.amplitude > 0.0
                    && (self.footwork.is_none() || (self.gait_ready && self.contact_clock.locked()))
                {
                    "dancing"
                } else if reason.is_some() {
                    "waiting"
                } else {
                    "acquiring"
                }
                .into(),
                bpm: frame.map(|f| f.bpm),
                reason: if self.footwork.as_ref().is_some_and(|f| f.halted()) {
                    Some("dance_area_limit".into())
                } else {
                    reason.map(str::to_owned)
                },
            },
        )
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn frame(at: Instant) -> MusicFrame {
        MusicFrame {
            at,
            music: true,
            bpm: 120.0,
            beat: 0.0,
            locked: true,
        }
    }
    fn warm(d: &mut Dance, now: Instant) {
        for i in 0..101 {
            let at = now + Duration::from_millis(i * 20);
            d.tick(at, Some(&frame(at)), true, true, Duration::MAX);
        }
    }
    #[test]
    fn footwork_owns_velocity_without_body_pose_and_manual_input_stops_it() {
        let now = Instant::now();
        let mut d = Dance::new(true);
        warm(&mut d, now);
        let mut moved = false;
        let mut stepped = false;
        for i in 1..=50 {
            let at = now + Duration::from_secs(2) + Duration::from_millis(i * 20);
            let mut f = frame(at);
            f.beat = (i as f64 * 0.04).fract();
            let (c, _) = d.tick(at, Some(&f), true, true, Duration::MAX);
            moved |= c.twist[0].hypot(c.twist[1]) > 0.001;
            assert_eq!(c.body, BodyPose::default());
            let mut targets = [0.0; 15];
            d.apply_footwork(&mut targets);
            stepped |= [1, 3, 11, 13].iter().any(|&j| targets[j].abs() > 0.01);
        }
        assert!(moved && stepped);
        assert!(d.walking());
        let at = now + Duration::from_secs(3);
        let (c, _) = d.tick(
            at + Duration::from_millis(20),
            Some(&frame(at)),
            true,
            true,
            Duration::ZERO,
        );
        assert_eq!(c, Command::default());
        assert!(!d.walking());
        let mut targets = [0.0; 15];
        d.apply_footwork(&mut targets);
        assert_eq!(targets, [0.0; 15]);
    }
    #[test]
    fn footwork_acquires_planted_standing_before_a_complete_first_swing() {
        let now = Instant::now();
        let mut d = Dance::new(true);
        let mut ready = false;
        for i in 0..180 {
            let at = now + Duration::from_millis(i * 20);
            let mut f = frame(at);
            f.beat = (i as f64 * 0.04).fract();
            let (c, status) = d.tick(at, Some(&f), true, true, Duration::MAX);
            if !d.gait_ready {
                assert_eq!(c.twist, [0.0; 3]);
                let mut q = [0.0; 15];
                d.apply_footwork(&mut q);
                for j in [1, 3, 11, 13] {
                    assert_eq!(q[j], 0.0);
                }
                assert_ne!(status.state, "dancing");
            } else if !ready {
                assert_eq!(d.amplitude, 1.0);
                assert!(d.planted(), "never join a swing halfway through");
                ready = true;
            }
        }
        assert!(ready);
    }
    #[test]
    fn walking_stays_inside_the_speed_circle_through_direction_changes() {
        let now = Instant::now();
        let mut d = Dance::new(true);
        for i in 0..1500 {
            let at = now + Duration::from_millis(i * 20);
            let mut f = frame(at);
            f.bpm = 150.0;
            f.beat = (i as f64 * 0.05).fract();
            let (c, _) = d.tick(at, Some(&f), true, true, Duration::MAX);
            assert!(c.twist[0].hypot(c.twist[1]) <= MAX_SPEED + 1e-12);
            assert!(c.twist[2].abs() <= crate::dance_motion::MAX_YAW_RATE);
        }
    }
    #[test]
    fn acquires_fades_and_reacquires() {
        let now = Instant::now();
        let mut d = Dance::default();
        warm(&mut d, now);
        assert_eq!(d.amplitude, 1.0);
        for i in 1..=50 {
            let at = now + Duration::from_secs(2) + Duration::from_millis(i * 20);
            let mut f = frame(at);
            f.music = false;
            d.tick(at, Some(&f), true, true, Duration::MAX);
        }
        assert!(d.amplitude < 1e-6);
        let at = now + Duration::from_secs(3);
        let (c, s) = d.tick(at, Some(&frame(at)), true, true, Duration::MAX);
        assert_eq!(c, Command::default());
        assert_eq!(s.state, "acquiring");
    }
    #[test]
    fn music_loss_finishes_the_clock_and_resets_the_previous_dance_area() {
        let now = Instant::now();
        let mut d = Dance::new(true);
        warm(&mut d, now);
        d.update_odometry([0.6, 0.0, 0.0]);
        let at = now + Duration::from_millis(2020);
        d.tick(at, Some(&frame(at)), true, true, Duration::MAX);
        assert!(d.footwork.as_ref().unwrap().halted());
        let phase = d.phase;
        for i in 1..=60 {
            let at = at + Duration::from_millis(i * 20);
            let mut f = frame(at);
            f.music = false;
            d.tick(at, Some(&f), true, true, Duration::MAX);
            if i == 1 {
                assert!(d.phase > phase, "music loss must not freeze a lifted foot");
            }
        }
        assert_eq!(d.amplitude, 0.0);
        assert!(!d.footwork.as_ref().unwrap().halted());
        // This is a new dance at the current location, not an escape from the old area.
        for i in 1..=150 {
            let at = now + Duration::from_millis(3220 + i * 20);
            d.tick(at, Some(&frame(at)), true, true, Duration::MAX);
        }
        assert!(!d.footwork.as_ref().unwrap().halted());
    }
    #[test]
    fn stale_and_nonfinite_audio_cannot_drive() {
        let now = Instant::now();
        let mut d = Dance::default();
        warm(&mut d, now);
        let (c, s) = d.tick(
            now + Duration::from_secs(3),
            Some(&frame(now)),
            true,
            true,
            Duration::MAX,
        );
        assert_eq!(c, Command::default());
        assert_eq!(s.reason.as_deref(), Some("audio_unavailable"));
        for value in [f64::NAN, f64::INFINITY] {
            let mut f = frame(now + Duration::from_secs(3));
            f.beat = value;
            let (c, _) = d.tick(f.at, Some(&f), true, true, Duration::MAX);
            assert_eq!(c, Command::default());
        }
    }
    #[test]
    fn manual_control_pauses_then_resumes_after_ten_seconds() {
        let now = Instant::now();
        let mut d = Dance::default();
        warm(&mut d, now);
        for i in 0..500 {
            let at = now + Duration::from_secs(2) + Duration::from_millis(i * 20);
            let (c, s) = d.tick(
                at,
                Some(&frame(at)),
                true,
                true,
                Duration::from_millis(i * 20),
            );
            assert_eq!(c, Command::default());
            assert_eq!(s.reason.as_deref(), Some("manual_control"));
        }
        let at = now + Duration::from_secs(12);
        let (_, s) = d.tick(at, Some(&frame(at)), true, true, Duration::from_secs(10));
        assert_eq!(s.state, "acquiring");
        warm(&mut d, at);
        assert_eq!(d.amplitude, 1.0);
    }
    #[test]
    fn disabling_revokes_immediately() {
        let now = Instant::now();
        let mut d = Dance::default();
        warm(&mut d, now);
        let at = now + Duration::from_secs(2);
        let (c, _) = d.tick(at, Some(&frame(at)), false, true, Duration::MAX);
        assert_eq!(c, Command::default());
    }
    #[test]
    fn manual_and_safety_take_ownership() {
        let mut d = Dance::default();
        let now = Instant::now();
        let f = MusicFrame {
            at: now,
            music: true,
            bpm: 120.0,
            beat: 0.0,
            locked: true,
        };
        for i in 0..100 {
            let t = now + Duration::from_millis(i * 20);
            let mut f = f.clone();
            f.at = t;
            d.tick(t, Some(&f), true, true, Duration::from_secs(11));
        }
        let (c, _) = d.tick(
            now + Duration::from_secs(2),
            Some(&f),
            true,
            false,
            Duration::from_secs(11),
        );
        assert_eq!(c, Command::default());
    }
    #[test]
    fn every_beat_has_one_dominant_nod_even_during_the_pose_hold() {
        for bpm in [70.0, 90.0, 120.0, 150.0, 160.0] {
            for beat in 0..32 {
                let on = rhythmic_command(beat as f64, bpm).head[1];
                let off = rhythmic_command(beat as f64 + 0.5, bpm).head[1];
                assert!(
                    on - off > 6f64.to_radians(),
                    "no pulse at beat {beat}, bpm {bpm}"
                );
                assert!((beat_pulse(beat as f64) - 1.0).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn phrases_have_opposition_a_dip_and_a_hold() {
        let left = choreography(0.0);
        let right = choreography(1.0);
        assert!(left.body.roll < 0.0 && left.head[2] > 0.0);
        assert!(right.body.roll > 0.0 && right.head[2] < 0.0);
        assert_eq!(choreography(6.0).body.pitch, BODY_PITCH_LIMIT.to_radians());
        assert_eq!(choreography(7.05), choreography(7.5));
        let mirror = choreography(8.0);
        assert_eq!(mirror.body.roll, -left.body.roll);
        assert_eq!(mirror.head[2], -left.head[2]);
    }

    #[test]
    fn phrase_boundaries_are_continuous() {
        for beat in 1..=32 {
            let before = choreography(beat as f64 - 1e-6);
            let after = choreography(beat as f64 + 1e-6);
            assert!((before.body.roll - after.body.roll).abs() < 1e-8);
            assert!((before.head[2] - after.head[2]).abs() < 1e-8);
            assert!((before.body.z - after.body.z).abs() < 1e-8);
        }
    }

    #[test]
    fn fast_music_and_phase_corrections_cannot_jump_targets() {
        let now = Instant::now();
        let mut d = Dance::default();
        let mut previous = Command::default();
        for i in 0..2000 {
            let at = now + Duration::from_millis(i * 20);
            let f = MusicFrame {
                at,
                music: true,
                bpm: 160.0,
                beat: (i as f64 * 0.02 * 160.0 / 60.0 + if i > 900 { 0.4 } else { 0.0 }).fract(),
                locked: true,
            };
            let (c, _) = d.tick(at, Some(&f), true, true, Duration::MAX);
            for (a, b) in c.head.iter().zip(previous.head) {
                assert!((a - b).abs() <= 140f64.to_radians() * 0.02 + 1e-10);
            }
            assert!((c.body.roll - previous.body.roll).abs() <= 70f64.to_radians() * 0.02 + 1e-10);
            assert!(
                (c.body.pitch - previous.body.pitch).abs() <= 50f64.to_radians() * 0.02 + 1e-10
            );
            previous = c;
        }
    }

    #[test]
    fn stays_in_place_with_bounded_expressive_targets() {
        let mut d = Dance::default();
        let now = Instant::now();
        for i in 0..500 {
            let t = now + Duration::from_millis(i * 20);
            let f = MusicFrame {
                at: t,
                music: true,
                bpm: 120.0,
                beat: (i as f64 * 0.04) % 1.0,
                locked: true,
            };
            let (c, _) = d.tick(t, Some(&f), true, true, Duration::from_secs(11));
            assert_eq!(c.twist, [0.0; 3]);
            assert_eq!(c.body.z, 0.0);
            assert!(c.body.roll.abs() <= BODY_ROLL_LIMIT.to_radians() + 1e-10);
            assert!(c.body.pitch.abs() <= BODY_PITCH_LIMIT.to_radians() + 1e-10);
            assert!(c.head[1].abs() <= HEAD_PITCH_LIMIT.to_radians() + 1e-10);
            assert!(c.head[2].abs() <= HEAD_YAW_LIMIT.to_radians() + 1e-10);
        }
    }
}
