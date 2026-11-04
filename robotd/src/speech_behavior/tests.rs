use super::*;

fn facts() -> Facts {
    Facts {
        edges: Edges::default(),
        fresh: true,
        down: false,
        still: false,
        battery_percent: Some(70.0),
        settled: true,
        available: true,
    }
}

#[test]
fn all_eight_triggers_have_twenty_unique_bounded_english_lines() {
    let now = Instant::now();
    let behavior = Behaviors::new(false, now, 42);
    let source: serde_json::Value =
        serde_json::from_str(include_str!("../../assets/speech-phrases.json")).unwrap();
    assert_eq!(source.as_object().unwrap().len(), Trigger::ALL.len());
    let mut unique = std::collections::HashSet::new();
    for trigger in Trigger::ALL {
        for text in &behavior.phrases[trigger as usize] {
            duck_speech::validate_text(text).unwrap();
            assert!(text.len() <= 120, "keep spontaneous speech short: {text}");
            assert!(unique.insert(text), "duplicate phrase: {text}");
        }
    }
    assert_eq!(unique.len(), 160);
}

#[test]
fn greeting_waits_briefly_for_the_worker_and_respects_its_switch() {
    let now = Instant::now();
    let mut b = Behaviors::new(true, now, 42);
    let busy = Facts {
        available: false,
        ..facts()
    };
    assert!(b.tick(now, busy).is_none());
    let hello = b.tick(now + Duration::from_secs(2), facts()).unwrap();
    assert_eq!(hello.trigger, Trigger::Greeting);
    b.admitted(hello, now + Duration::from_secs(2));
    assert!(b.tick(now + Duration::from_secs(12), facts()).is_none());
    assert!(Behaviors::new(false, now, 42).tick(now, facts()).is_none());
    let mut expired = Behaviors::new(true, now, 42);
    assert!(
        expired
            .tick(now + Duration::from_secs(16), facts())
            .is_none()
    );
}

#[test]
fn transient_events_have_cooldowns_and_never_wait_behind_other_audio() {
    let now = Instant::now();
    let mut b = Behaviors::new(false, now, 42);
    let pet = || Facts {
        edges: Edges {
            petted: true,
            ..Default::default()
        },
        ..facts()
    };
    let first = b.tick(now, pet()).unwrap();
    assert_eq!(first.trigger, Trigger::Petting);
    b.admitted(first, now);
    assert!(b.tick(now + Duration::from_secs(10), pet()).is_none());
    assert!(
        b.tick(
            now + Duration::from_secs(31),
            Facts {
                available: false,
                ..pet()
            }
        )
        .is_none()
    );
    assert!(
        b.tick(now + Duration::from_secs(32), facts()).is_none(),
        "stale pet queued"
    );
    for (seconds, edges, expected) in [
        (
            40,
            Edges {
                picked_up: true,
                ..Default::default()
            },
            Trigger::PickedUp,
        ),
        (
            44,
            Edges {
                put_down: true,
                ..Default::default()
            },
            Trigger::PutDown,
        ),
        (
            49,
            Edges {
                put_down: true,
                ..Default::default()
            },
            Trigger::PutDown,
        ),
    ] {
        let time = now + Duration::from_secs(seconds);
        let result = b.tick(time, Facts { edges, ..facts() });
        if seconds == 44 {
            assert!(result.is_none(), "global gap was bypassed");
        } else {
            let utterance = result.unwrap();
            assert_eq!(utterance.trigger, expected);
            b.admitted(utterance, time);
        }
    }
}

#[test]
fn random_selection_never_repeats_the_previous_line_for_that_trigger() {
    let now = Instant::now();
    let mut b = Behaviors::new(false, now, 42);
    let mut previous = None;
    for n in 0..200 {
        let time = now + Duration::from_secs(n * 31);
        let utterance = b
            .tick(
                time,
                Facts {
                    edges: Edges {
                        petted: true,
                        ..Default::default()
                    },
                    ..facts()
                },
            )
            .unwrap();
        assert!(utterance.index < 20);
        assert_ne!(previous, Some(utterance.index));
        previous = Some(utterance.index);
        b.admitted(utterance, time);
    }
}

#[test]
fn low_battery_waits_for_safe_audio_then_announces_once_until_recharged() {
    let now = Instant::now();
    let mut b = Behaviors::new(false, now, 42);
    for percent in [None, Some(f64::NAN), Some(-1.0), Some(21.0)] {
        assert!(
            b.tick(
                now,
                Facts {
                    battery_percent: percent,
                    ..facts()
                }
            )
            .is_none()
        );
    }
    let low = || Facts {
        battery_percent: Some(20.0),
        ..facts()
    };
    assert!(
        b.tick(
            now,
            Facts {
                available: false,
                ..low()
            }
        )
        .is_none()
    );
    let utterance = b.tick(now + Duration::from_secs(5), low()).unwrap();
    assert_eq!(utterance.trigger, Trigger::LowBattery);
    b.admitted(utterance, now + Duration::from_secs(5));
    assert!(b.tick(now + Duration::from_secs(100), low()).is_none());
    assert!(
        b.tick(
            now + Duration::from_secs(101),
            Facts {
                battery_percent: Some(25.0),
                ..facts()
            }
        )
        .is_none()
    );
    assert!(b.tick(now + Duration::from_secs(102), low()).is_none());
    assert!(
        b.tick(
            now + Duration::from_secs(103),
            Facts {
                battery_percent: Some(30.0),
                ..facts()
            }
        )
        .is_none()
    );
    assert_eq!(
        b.tick(now + Duration::from_secs(104), low())
            .unwrap()
            .trigger,
        Trigger::LowBattery
    );
}

#[test]
fn idle_speaks_once_per_spell_and_wake_requires_a_real_long_idle() {
    let now = Instant::now();
    let mut b = Behaviors::new(false, now, 42);
    let still = || Facts {
        still: true,
        ..facts()
    };
    assert!(b.tick(now, still()).is_none());
    assert!(b.tick(now + Duration::from_secs(89), still()).is_none());
    let idle = b.tick(now + Duration::from_secs(90), still()).unwrap();
    assert_eq!(idle.trigger, Trigger::Idle);
    b.admitted(idle, now + Duration::from_secs(90));
    assert!(b.tick(now + Duration::from_secs(400), still()).is_none());
    let wake = b.tick(now + Duration::from_secs(401), facts()).unwrap();
    assert_eq!(wake.trigger, Trigger::Wake);
    b.admitted(wake, now + Duration::from_secs(401));
    assert!(b.tick(now + Duration::from_secs(410), facts()).is_none());
    assert!(b.tick(now + Duration::from_secs(420), still()).is_none());
    assert!(b.tick(now + Duration::from_secs(421), facts()).is_none());
    b.tick(now + Duration::from_secs(500), still());
    assert!(
        b.tick(
            now + Duration::from_secs(600),
            Facts {
                fresh: false,
                ..facts()
            }
        )
        .is_none()
    );
    assert!(
        b.tick(now + Duration::from_secs(601), facts()).is_none(),
        "a sensor gap became a wake"
    );
}

#[test]
fn recovery_requires_fresh_stable_upright_readings_and_does_not_queue() {
    let now = Instant::now();
    let mut b = Behaviors::new(false, now, 42);
    assert!(
        b.tick(
            now,
            Facts {
                down: true,
                ..facts()
            }
        )
        .is_none()
    );
    assert!(b.tick(now + Duration::from_secs(1), facts()).is_none());
    assert!(
        b.tick(
            now + Duration::from_secs(2),
            Facts {
                fresh: false,
                ..facts()
            }
        )
        .is_none()
    );
    assert!(b.tick(now + Duration::from_secs(4), facts()).is_none());
    let recovered = b.tick(now + Duration::from_secs(6), facts()).unwrap();
    assert_eq!(recovered.trigger, Trigger::Recovered);
    b.admitted(recovered, now + Duration::from_secs(6));
    assert!(b.tick(now + Duration::from_secs(100), facts()).is_none());
    b.tick(
        now + Duration::from_secs(101),
        Facts {
            down: true,
            ..facts()
        },
    );
    b.tick(now + Duration::from_secs(102), facts());
    assert!(
        b.tick(
            now + Duration::from_secs(104),
            Facts {
                available: false,
                ..facts()
            }
        )
        .is_none()
    );
    assert!(b.tick(now + Duration::from_secs(105), facts()).is_none());
}

#[test]
fn recovery_waits_for_motion_ownership_to_settle_before_its_stability_timer() {
    let now = Instant::now();
    let mut b = Behaviors::new(false, now, 42);
    b.tick(
        now,
        Facts {
            down: true,
            ..facts()
        },
    );
    assert!(b.tick(now + Duration::from_secs(1), facts()).is_none());
    assert!(
        b.tick(
            now + Duration::from_secs(4),
            Facts {
                settled: false,
                ..facts()
            }
        )
        .is_none()
    );
    assert!(b.tick(now + Duration::from_secs(10), facts()).is_none());
    assert_eq!(
        b.tick(now + Duration::from_secs(12), facts())
            .unwrap()
            .trigger,
        Trigger::Recovered
    );
}

#[test]
fn priority_does_not_turn_a_dropped_edge_into_a_delayed_reaction() {
    let now = Instant::now();
    let mut b = Behaviors::new(false, now, 42);
    let utterance = b
        .tick(
            now,
            Facts {
                edges: Edges {
                    picked_up: true,
                    petted: true,
                    ..Default::default()
                },
                battery_percent: Some(10.0),
                ..facts()
            },
        )
        .unwrap();
    assert_eq!(utterance.trigger, Trigger::LowBattery);
    b.admitted(utterance, now);
    assert!(b.tick(now + Duration::from_secs(10), facts()).is_none());
}

#[test]
#[ignore = "requires MICRODUCK_SPEECH_MODEL and ONNX Runtime; synthesizes all 160 lines"]
fn every_behavior_line_synthesizes_with_the_native_heart_engine() {
    let path = std::env::var_os("MICRODUCK_SPEECH_MODEL").expect("set MICRODUCK_SPEECH_MODEL");
    let mut engine = duck_speech::Engine::new(std::path::Path::new(&path)).unwrap();
    let behavior = Behaviors::new(false, Instant::now(), 42);
    for trigger in Trigger::ALL {
        for text in &behavior.phrases[trigger as usize] {
            let audio = engine
                .synthesize(text, &Default::default())
                .unwrap_or_else(|e| panic!("{}: {text}: {e}", trigger.name()));
            assert!(
                audio.pcm.iter().any(|v| v.abs() > 0.01),
                "silent line: {text}"
            );
            assert!(
                audio.mouth.duration() < Duration::from_secs(10),
                "long reaction: {text}"
            );
        }
    }
}
