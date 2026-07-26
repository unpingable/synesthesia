use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crossbeam_channel::{Receiver, Sender, TrySendError, bounded};
use thiserror::Error;

use crate::{
    event::NormalizedEvent,
    flight_recorder::{
        FlightConfig, FlightError, FlightRecorder, FlightState, FlightStatus, IncidentLosses,
        Termination,
    },
    trigger::{TriggerError, TriggerEvaluator, TriggerSpec},
};

pub const FLIGHT_WRITER_CHANNEL_CAPACITY: usize = 4_096;
const TICK_INTERVAL: Duration = Duration::from_millis(25);

enum Command {
    Event {
        event: NormalizedEvent,
        now: f64,
        losses: IncidentLosses,
    },
    Losses(IncidentLosses),
    Manual,
    Cancel,
    Interrupt,
    SourceFailed,
    RendererFailed,
}

struct Shared {
    status: Mutex<FlightStatus>,
    error: Mutex<Option<String>>,
    published: AtomicBool,
    writer_drops: AtomicU64,
}

#[derive(Clone)]
pub struct FlightRuntimeSender {
    commands: Sender<Command>,
    shared: Arc<Shared>,
    started: Instant,
}

impl FlightRuntimeSender {
    pub fn observe(&self, event: NormalizedEvent, losses: IncidentLosses) {
        let command = Command::Event {
            event,
            now: self.started.elapsed().as_secs_f64(),
            losses,
        };
        match self.commands.try_send(command) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                self.shared.writer_drops.fetch_add(1, Ordering::Relaxed);
            }
            Err(TrySendError::Disconnected(_)) => {}
        }
    }

    pub fn source_failed(&self) {
        let _ = self.commands.try_send(Command::SourceFailed);
    }

    pub fn update_losses(&self, losses: IncidentLosses) {
        let _ = self.commands.try_send(Command::Losses(losses));
    }
}

pub struct FlightRuntime {
    commands: Sender<Command>,
    shared: Arc<Shared>,
    started: Instant,
    join: Option<JoinHandle<()>>,
}

impl FlightRuntime {
    pub fn spawn(config: FlightConfig, trigger: TriggerSpec) -> Result<Self, FlightRuntimeError> {
        let mut recorder = FlightRecorder::new(config)?;
        recorder.arm(0.0)?;
        let evaluator = TriggerEvaluator::new(trigger, config_source(&recorder))?;
        let status = recorder.status(0.0);
        let (commands, receiver) = bounded(FLIGHT_WRITER_CHANNEL_CAPACITY);
        let shared = Arc::new(Shared {
            status: Mutex::new(status),
            error: Mutex::new(None),
            published: AtomicBool::new(false),
            writer_drops: AtomicU64::new(0),
        });
        let started = Instant::now();
        let worker_shared = Arc::clone(&shared);
        let join = thread::Builder::new()
            .name("synesthesia-flight-recorder".to_owned())
            .spawn(move || run_worker(recorder, evaluator, receiver, worker_shared, started))
            .map_err(FlightRuntimeError::Spawn)?;
        Ok(Self {
            commands,
            shared,
            started,
            join: Some(join),
        })
    }

    pub fn sender(&self) -> FlightRuntimeSender {
        FlightRuntimeSender {
            commands: self.commands.clone(),
            shared: Arc::clone(&self.shared),
            started: self.started,
        }
    }

    pub fn status(&self) -> FlightStatus {
        self.shared
            .status
            .lock()
            .expect("flight status lock")
            .clone()
    }

    pub fn published(&self) -> bool {
        self.shared.published.load(Ordering::Acquire)
    }

    pub fn error(&self) -> Option<String> {
        self.shared.error.lock().expect("flight error lock").clone()
    }

    pub fn manual_trigger(&self) -> Result<(), FlightRuntimeError> {
        self.send_control(Command::Manual)
    }

    pub fn cancel(&self) -> Result<(), FlightRuntimeError> {
        self.send_control(Command::Cancel)
    }

    pub fn interrupt(&self) -> Result<(), FlightRuntimeError> {
        self.send_control(Command::Interrupt)
    }

    pub fn renderer_failed(&self) -> Result<(), FlightRuntimeError> {
        self.send_control(Command::RendererFailed)
    }

    fn send_control(&self, command: Command) -> Result<(), FlightRuntimeError> {
        self.commands
            .send_timeout(command, Duration::from_millis(100))
            .map_err(|_| FlightRuntimeError::ControlUnavailable)
    }
}

impl Drop for FlightRuntime {
    fn drop(&mut self) {
        if !matches!(
            self.status().state,
            FlightState::Complete | FlightState::Cancelled | FlightState::Failed
        ) {
            let _ = self
                .commands
                .send_timeout(Command::Interrupt, Duration::from_millis(100));
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn config_source(recorder: &FlightRecorder) -> crate::flight_recorder::FlightSource {
    recorder.source()
}

fn run_worker(
    mut recorder: FlightRecorder,
    mut evaluator: TriggerEvaluator,
    receiver: Receiver<Command>,
    shared: Arc<Shared>,
    started: Instant,
) {
    let mut losses = IncidentLosses::default();
    let mut last_event: Option<(f64, Option<f64>)> = None;
    loop {
        let now = started.elapsed().as_secs_f64();
        let result = match receiver.recv_timeout(TICK_INTERVAL) {
            Ok(Command::Event {
                event,
                now,
                losses: incoming_losses,
            }) => {
                losses = incoming_losses;
                losses.writer = shared.writer_drops.load(Ordering::Relaxed);
                let timestamp = event.timestamp;
                let trigger = if recorder.state() == FlightState::Armed {
                    evaluator.observe(&event, now)
                } else {
                    None
                };
                last_event = Some((now, timestamp));
                recorder.observe(event, now, losses).and_then(|complete| {
                    if complete {
                        Ok(true)
                    } else if let Some(trigger) = trigger {
                        recorder.trigger(now, timestamp, trigger, losses)
                    } else {
                        Ok(false)
                    }
                })
            }
            Ok(Command::Losses(incoming_losses)) => {
                losses = incoming_losses;
                Ok(false)
            }
            Ok(Command::Manual) => {
                let timestamp = estimated_timestamp(last_event, now);
                evaluator.manual().map_or(Ok(false), |trigger| {
                    recorder.trigger(now, timestamp, trigger, losses_with_writer(losses, &shared))
                })
            }
            Ok(Command::Cancel) => match recorder.state() {
                FlightState::Armed => recorder.cancel().map(|()| false),
                FlightState::CapturingTail => {
                    recorder.interrupt(now, losses_with_writer(losses, &shared))
                }
                state => Err(FlightError::IllegalTransition {
                    state,
                    operation: "cancel",
                }),
            },
            Ok(Command::Interrupt) => recorder.interrupt(now, losses_with_writer(losses, &shared)),
            Ok(Command::SourceFailed) => {
                *shared.error.lock().expect("flight error lock") =
                    Some("live collector exited unexpectedly".to_owned());
                recorder.fail_after_trigger(
                    now,
                    losses_with_writer(losses, &shared),
                    Termination::CollectorError,
                )
            }
            Ok(Command::RendererFailed) => recorder.fail_after_trigger(
                now,
                losses_with_writer(losses, &shared),
                Termination::RendererError,
            ),
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                if recorder.state() == FlightState::Armed {
                    evaluator.tick(now).map_or(Ok(false), |trigger| {
                        recorder.trigger(
                            now,
                            estimated_timestamp(last_event, now),
                            trigger,
                            losses_with_writer(losses, &shared),
                        )
                    })
                } else {
                    recorder.tick(now, losses_with_writer(losses, &shared))
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                recorder.interrupt(now, losses_with_writer(losses, &shared))
            }
        };
        update_status(&shared, &recorder, now);
        match result {
            Ok(published) => {
                if published {
                    shared.published.store(true, Ordering::Release);
                    break;
                }
                if matches!(
                    recorder.state(),
                    FlightState::Cancelled | FlightState::Failed
                ) {
                    break;
                }
            }
            Err(error) => {
                *shared.error.lock().expect("flight error lock") = Some(error.to_string());
                update_status(&shared, &recorder, now);
                break;
            }
        }
    }
}

fn losses_with_writer(mut losses: IncidentLosses, shared: &Shared) -> IncidentLosses {
    losses.writer = shared.writer_drops.load(Ordering::Relaxed);
    losses
}

fn estimated_timestamp(last: Option<(f64, Option<f64>)>, now: f64) -> Option<f64> {
    last.and_then(|(observed, timestamp)| timestamp.map(|value| value + (now - observed).max(0.0)))
}

fn update_status(shared: &Shared, recorder: &FlightRecorder, now: f64) {
    *shared.status.lock().expect("flight status lock") = recorder.status(now);
}

#[derive(Debug, Error)]
pub enum FlightRuntimeError {
    #[error(transparent)]
    Recorder(#[from] FlightError),
    #[error(transparent)]
    Trigger(#[from] TriggerError),
    #[error("could not spawn flight-recorder writer: {0}")]
    Spawn(std::io::Error),
    #[error("flight-recorder control channel is unavailable")]
    ControlUnavailable,
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use crate::{
        event::{Direction, SCHEMA_VERSION},
        flight_recorder::{FlightSource, HostMetadata, part_path},
    };

    use super::*;

    fn path(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("synesthesia-runtime-{label}-{unique}.ndjson"))
    }

    fn config(path: PathBuf) -> FlightConfig {
        FlightConfig {
            output: path,
            source: FlightSource::Tcp,
            pre_trigger: Duration::from_secs(1),
            post_trigger: Duration::ZERO,
            max_events: 64,
            max_bytes: 64 * 1024,
            host: HostMetadata {
                kernel: "test".to_owned(),
                architecture: "test".to_owned(),
            },
        }
    }

    fn event() -> NormalizedEvent {
        NormalizedEvent {
            v: SCHEMA_VERSION,
            timestamp: Some(1.0),
            category: "tcp.retransmit".to_owned(),
            origin: Some("192.0.2.1:1234".to_owned()),
            target: Some("198.51.100.2:443".to_owned()),
            magnitude: 100.0,
            direction: Direction::Outbound,
            labels: BTreeMap::new(),
        }
    }

    #[test]
    fn bounded_writer_channel_counts_refused_events() {
        let (commands, _receiver) = bounded(FLIGHT_WRITER_CHANNEL_CAPACITY);
        let output = path("bounded");
        let shared = Arc::new(Shared {
            status: Mutex::new(FlightRecorder::new(config(output)).unwrap().status(0.0)),
            error: Mutex::new(None),
            published: AtomicBool::new(false),
            writer_drops: AtomicU64::new(0),
        });
        let sender = FlightRuntimeSender {
            commands,
            shared: Arc::clone(&shared),
            started: Instant::now(),
        };
        for _ in 0..=FLIGHT_WRITER_CHANNEL_CAPACITY {
            sender.observe(event(), IncidentLosses::default());
        }
        assert_eq!(shared.writer_drops.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn cancellation_does_not_publish_or_leave_a_partial_file() {
        let output = path("cancel");
        let runtime = FlightRuntime::spawn(config(output.clone()), TriggerSpec::Manual).unwrap();
        runtime.cancel().unwrap();
        while runtime.status().state == FlightState::Armed {
            thread::yield_now();
        }
        drop(runtime);
        assert!(!output.exists());
        assert!(!part_path(&output).exists());
    }

    #[test]
    fn immediate_tcp_reset_trigger_publishes_through_the_worker() {
        let output = path("tcp-reset");
        let runtime = FlightRuntime::spawn(config(output.clone()), TriggerSpec::TcpReset).unwrap();
        let sender = runtime.sender();
        let mut reset = event();
        reset.category = "tcp.reset.receive".to_owned();
        reset.direction = Direction::Inbound;
        sender.observe(reset, IncidentLosses::default());
        let deadline = Instant::now() + Duration::from_secs(2);
        while !runtime.published() && runtime.error().is_none() && Instant::now() < deadline {
            thread::yield_now();
        }
        assert!(runtime.published(), "{:?}", runtime.error());
        let records: Vec<NormalizedEvent> = std::fs::read_to_string(&output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert!(
            records
                .iter()
                .any(|record| record.category == crate::flight_recorder::TRIGGER_CATEGORY)
        );
        assert_eq!(records.last().unwrap().labels["termination"], "normal");
        std::fs::remove_file(output).unwrap();
    }
}
