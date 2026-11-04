//! Background text-to-speech. A request owns its reply slot, so cancellation cannot queue stale audio.
use anyhow::{Result, bail};
use duck_speech::{Audio, Cancellation, Engine};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
};

const STARTING: u8 = 0;
const READY: u8 = 1;
const FAILED: u8 = 2;
struct Request {
    text: String,
    cancel: Cancellation,
    reply: SyncSender<Result<Audio>>,
}

pub struct Pending {
    pub reply: Receiver<Result<Audio>>,
    cancel: Cancellation,
}
impl Drop for Pending {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

pub struct Worker {
    requests: SyncSender<Request>,
    status: Arc<AtomicU8>,
    busy: Arc<AtomicBool>,
}
impl Worker {
    pub fn spawn(model: PathBuf, nasal: bool) -> Result<Self> {
        let (tx, rx) = mpsc::sync_channel::<Request>(1);
        let status = Arc::new(AtomicU8::new(STARTING));
        let busy = Arc::new(AtomicBool::new(false));
        let worker_status = status.clone();
        let worker_busy = busy.clone();
        std::thread::Builder::new()
            .name("speech".into())
            .spawn(move || {
                // Linux niceness is per thread. Synthesis must yield CPU to the motor loop.
                #[cfg(target_os = "linux")]
                unsafe {
                    libc::setpriority(libc::PRIO_PROCESS, 0, 10);
                }
                let initialized = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    Engine::with_nasal(&model, nasal)
                }));
                let mut engine = match initialized {
                    Ok(Ok(engine)) => engine,
                    result => {
                        let error = match result {
                            Ok(Err(e)) => e.to_string(),
                            _ => "speech initialization panicked".into(),
                        };
                        tracing::warn!(%error, model=%model.display(), "on-device speech unavailable");
                        worker_status.store(FAILED, Ordering::Release);
                        return;
                    }
                };
                worker_status.store(READY, Ordering::Release);
                tracing::info!(voice=duck_speech::VOICE, "on-device speech ready");
                while let Ok(request) = rx.recv() {
                    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        engine.synthesize(&request.text, &request.cancel)
                    }));
                    let panic = outcome.is_err();
                    let result = outcome
                        .unwrap_or_else(|_| Err(anyhow::anyhow!("speech synthesis panicked")));
                    // A canceled caller has dropped its receiver. No queue feeds its successor.
                    let _ = request.reply.try_send(result);
                    worker_busy.store(false, Ordering::Release);
                    if panic {
                        worker_status.store(FAILED, Ordering::Release);
                        break;
                    }
                }
            })?;
        Ok(Self {
            requests: tx,
            status,
            busy,
        })
    }
    pub fn ready(&self) -> bool {
        self.status.load(Ordering::Acquire) == READY
    }
    pub fn available(&self) -> bool {
        self.ready() && !self.busy.load(Ordering::Acquire)
    }
    pub fn request(&self, text: String) -> Result<Pending> {
        if !self.ready() {
            bail!("speech_not_ready");
        }
        if self
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            bail!("speech_busy");
        }
        let (tx, rx) = mpsc::sync_channel(1);
        let cancel = Cancellation::default();
        if self
            .requests
            .try_send(Request {
                text,
                cancel: cancel.clone(),
                reply: tx,
            })
            .is_err()
        {
            self.busy.store(false, Ordering::Release);
            bail!("speech_worker_unavailable");
        }
        Ok(Pending { reply: rx, cancel })
    }
}

/// Audio and beak have separate gates: torque-off audio must not animate a servo.
#[derive(Default, Clone, Copy)]
pub struct Conditions {
    pub fallen: bool,
    pub limp_fall: bool,
    pub shutdown: bool,
    pub rest: bool,
    pub relax: bool,
    pub enabled: bool,
    pub ready: bool,
    pub fresh: bool,
    pub imu_ready: bool,
    pub skill_busy: bool,
    pub instrument_busy: bool,
    pub transitioning: bool,
}
impl Conditions {
    pub fn audio_allowed(self) -> bool {
        !self.fallen && !self.limp_fall && !self.shutdown && !self.rest && !self.relax
    }
    pub fn mouth_allowed(self) -> bool {
        self.audio_allowed()
            && self.enabled
            && self.ready
            && self.fresh
            && self.imu_ready
            && !self.skill_busy
            && !self.instrument_busy
            && !self.transitioning
    }
}

#[cfg(test)]
impl Pending {
    pub fn from_reply(reply: Receiver<Result<Audio>>) -> Self {
        Self {
            reply,
            cancel: Cancellation::default(),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_assets_do_not_make_an_optional_worker_ready() {
        let worker =
            Worker::spawn(PathBuf::from("/nonexistent/microduck-speech.onnx"), true).unwrap();
        let end = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while worker.status.load(Ordering::Acquire) == STARTING {
            assert!(std::time::Instant::now() < end);
            std::thread::yield_now();
        }
        assert!(!worker.available());
        assert!(worker.request("Hello".into()).is_err());
    }
    #[test]
    fn safety_gates_audio_and_servos_independently() {
        let safe = Conditions {
            enabled: true,
            ready: true,
            fresh: true,
            imu_ready: true,
            ..Default::default()
        };
        assert!(safe.audio_allowed() && safe.mouth_allowed());
        for state in [
            Conditions {
                fallen: true,
                ..safe
            },
            Conditions {
                limp_fall: true,
                ..safe
            },
            Conditions {
                shutdown: true,
                ..safe
            },
            Conditions { rest: true, ..safe },
            Conditions {
                relax: true,
                ..safe
            },
        ] {
            assert!(!state.audio_allowed() && !state.mouth_allowed());
        }
        for state in [
            Conditions {
                enabled: false,
                ..safe
            },
            Conditions {
                ready: false,
                ..safe
            },
            Conditions {
                fresh: false,
                ..safe
            },
            Conditions {
                imu_ready: false,
                ..safe
            },
            Conditions {
                skill_busy: true,
                ..safe
            },
            Conditions {
                instrument_busy: true,
                ..safe
            },
            Conditions {
                transitioning: true,
                ..safe
            },
        ] {
            assert!(state.audio_allowed() && !state.mouth_allowed());
        }
    }
}
