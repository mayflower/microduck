//! Behavior events select text; the existing speech worker and player own synthesis and audio.
//! The contract and cooldowns live in docs/design/speech.md.
use std::time::{Duration, Instant};

use rand::{Rng, SeedableRng, rngs::StdRng};

const GAP: Duration = Duration::from_secs(8);
const IDLE_AFTER: Duration = Duration::from_secs(90);
const RECOVERY_STABLE: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    Greeting,
    Petting,
    PickedUp,
    PutDown,
    LowBattery,
    Idle,
    Wake,
    Recovered,
}
impl Trigger {
    const ALL: [Self; 8] = [
        Self::Greeting,
        Self::Petting,
        Self::PickedUp,
        Self::PutDown,
        Self::LowBattery,
        Self::Idle,
        Self::Wake,
        Self::Recovered,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::Greeting => "greeting",
            Self::Petting => "petting",
            Self::PickedUp => "picked_up",
            Self::PutDown => "put_down",
            Self::LowBattery => "low_battery",
            Self::Idle => "idle",
            Self::Wake => "wake",
            Self::Recovered => "recovered",
        }
    }
    fn cooldown(self) -> Duration {
        Duration::from_secs(match self {
            Self::Greeting => 0,
            Self::Petting => 30,
            Self::PickedUp | Self::PutDown => 20,
            Self::LowBattery | Self::Recovered => 60,
            Self::Idle => 180,
            Self::Wake => 45,
        })
    }
}

/// Edges from the existing detectors, consumed in the tick that observed them.
#[derive(Default)]
pub struct Edges {
    pub petted: bool,
    pub picked_up: bool,
    pub put_down: bool,
}

pub struct Facts {
    pub edges: Edges,
    pub fresh: bool,
    pub down: bool,
    pub still: bool,
    pub battery_percent: Option<f64>,
    /// No safety sequence, homing, skill, instrument or policy transition owns the robot.
    pub settled: bool,
    /// Current safety, audio ownership and manual-request priority have already been checked.
    pub available: bool,
}

#[derive(Clone, Copy)]
pub struct Utterance {
    pub trigger: Trigger,
    index: usize,
}

pub struct Behaviors {
    phrases: [[String; 20]; 8],
    rng: StdRng,
    previous: [Option<usize>; 8],
    last: [Option<Instant>; 8],
    last_any: Option<Instant>,
    greeting_until: Option<Instant>,
    low_announced: bool,
    idle_since: Option<Instant>,
    idle_announced: bool,
    recovering: bool,
    upright_since: Option<Instant>,
}
impl Behaviors {
    pub fn new(greet: bool, now: Instant, seed: u64) -> Self {
        // Parse only at startup, outside the motor loop. The embedded catalog is checked by tests.
        let mut catalog: std::collections::BTreeMap<String, Vec<String>> =
            serde_json::from_str(include_str!("../../assets/speech-phrases.json"))
                .expect("embedded speech catalog");
        let phrases = Trigger::ALL.map(|trigger| {
            catalog
                .remove(trigger.name())
                .expect("every speech trigger has a catalog")
                .try_into()
                .expect("exactly twenty phrases per trigger")
        });
        Self {
            phrases,
            rng: StdRng::seed_from_u64(seed),
            previous: [None; 8],
            last: [None; 8],
            last_any: None,
            greeting_until: greet.then_some(now + Duration::from_secs(15)),
            low_announced: false,
            idle_since: None,
            idle_announced: false,
            recovering: false,
            upright_since: None,
        }
    }

    /// Update observation history even while busy. No pet, pickup, wake or recovery is queued.
    pub fn tick(&mut self, now: Instant, facts: Facts) -> Option<Utterance> {
        let percent = facts
            .battery_percent
            .filter(|p| p.is_finite() && (0.0..=100.0).contains(p));
        if percent.is_some_and(|p| p >= 30.0) {
            self.low_announced = false;
        }
        let low = percent.is_some_and(|p| p <= 20.0) && !self.low_announced;

        let mut recovered = false;
        if facts.down {
            self.recovering = true;
            self.upright_since = None;
        } else if !facts.fresh || !facts.settled {
            self.upright_since = None;
        } else if self.recovering {
            let since = *self.upright_since.get_or_insert(now);
            if now.saturating_duration_since(since) >= RECOVERY_STABLE {
                recovered = true;
                self.recovering = false;
            }
        }

        let mut wake = false;
        if !facts.fresh || facts.down || !facts.settled {
            self.idle_since = None;
            self.idle_announced = false;
        } else if facts.still {
            self.idle_since.get_or_insert(now);
        } else if let Some(since) = self.idle_since.take() {
            wake = now.saturating_duration_since(since) >= IDLE_AFTER;
            self.idle_announced = false;
        }
        let idle = !self.idle_announced
            && self
                .idle_since
                .is_some_and(|since| now.saturating_duration_since(since) >= IDLE_AFTER);
        let greeting = self.greeting_until.is_some_and(|end| now <= end);
        if !greeting {
            self.greeting_until = None;
        }
        if !facts.available || !facts.settled || !facts.fresh || facts.down {
            return None;
        }
        if self
            .last_any
            .is_some_and(|last| now.saturating_duration_since(last) < GAP)
        {
            return None;
        }

        let candidates = [
            (low, Trigger::LowBattery),
            (recovered, Trigger::Recovered),
            (facts.edges.picked_up, Trigger::PickedUp),
            (facts.edges.put_down, Trigger::PutDown),
            (facts.edges.petted, Trigger::Petting),
            (wake, Trigger::Wake),
            (greeting, Trigger::Greeting),
            (idle, Trigger::Idle),
        ];
        let trigger = candidates.into_iter().find_map(|(happened, trigger)| {
            (happened
                && self.last[trigger as usize]
                    .is_none_or(|last| now.saturating_duration_since(last) >= trigger.cooldown()))
            .then_some(trigger)
        })?;
        let previous = self.previous[trigger as usize];
        // Draw uniformly from all twenty, then from the nineteen other choices after that.
        let pick = self
            .rng
            .random_range(0..if previous.is_some() { 19 } else { 20 });
        let index = pick + usize::from(previous.is_some_and(|last| pick >= last));
        Some(Utterance { trigger, index })
    }

    pub fn text(&self, utterance: Utterance) -> &str {
        &self.phrases[utterance.trigger as usize][utterance.index]
    }

    /// Only successful admission consumes a cooldown or a one-shot announcement.
    pub fn admitted(&mut self, utterance: Utterance, now: Instant) {
        let trigger = utterance.trigger;
        self.previous[trigger as usize] = Some(utterance.index);
        self.last[trigger as usize] = Some(now);
        self.last_any = Some(now);
        match trigger {
            Trigger::Greeting => self.greeting_until = None,
            Trigger::LowBattery => self.low_announced = true,
            Trigger::Idle => self.idle_announced = true,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;
