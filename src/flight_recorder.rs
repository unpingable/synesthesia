use std::{
    collections::{BTreeMap, VecDeque},
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::event::{Direction, NormalizedEvent, SCHEMA_VERSION};

pub const FLIGHT_FORMAT_VERSION: u16 = 1;
pub const DEFAULT_PRE_TRIGGER: Duration = Duration::from_secs(10);
pub const DEFAULT_POST_TRIGGER: Duration = Duration::from_secs(5);
pub const HARD_MAX_PRE_TRIGGER: Duration = Duration::from_secs(30);
pub const HARD_MAX_POST_TRIGGER: Duration = Duration::from_secs(30);
pub const HARD_MAX_EVENTS: usize = 100_000;
pub const HARD_MAX_BYTES: usize = 32 * 1024 * 1024;
pub const METADATA_CATEGORY: &str = "synesthesia.flight.metadata";
pub const TRIGGER_CATEGORY: &str = "synesthesia.flight.trigger";
pub const PHASE_LABEL: &str = "synesthesia.flight.phase";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FlightSource {
    Scheduler,
    Tcp,
}

impl FlightSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Scheduler => "scheduler",
            Self::Tcp => "tcp",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FlightState {
    Disarmed,
    Armed,
    CapturingTail,
    Complete,
    Cancelled,
    Failed,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct IncidentLosses {
    pub kernel_ring: u64,
    pub collector: u64,
    pub ipc: u64,
    pub renderer_channel: u64,
    pub malformed: u64,
    pub writer: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostMetadata {
    pub kernel: String,
    pub architecture: String,
}

impl HostMetadata {
    pub fn local() -> Self {
        let kernel = fs::read_to_string("/proc/sys/kernel/osrelease")
            .map(|value| value.trim().to_owned())
            .unwrap_or_else(|_| "unknown".to_owned());
        Self {
            kernel,
            architecture: std::env::consts::ARCH.to_owned(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct FlightConfig {
    pub output: PathBuf,
    pub source: FlightSource,
    pub pre_trigger: Duration,
    pub post_trigger: Duration,
    pub max_events: usize,
    pub max_bytes: usize,
    pub host: HostMetadata,
}

impl FlightConfig {
    pub fn new(output: PathBuf, source: FlightSource) -> Self {
        Self {
            output,
            source,
            pre_trigger: DEFAULT_PRE_TRIGGER,
            post_trigger: DEFAULT_POST_TRIGGER,
            max_events: HARD_MAX_EVENTS,
            max_bytes: HARD_MAX_BYTES,
            host: HostMetadata::local(),
        }
    }

    fn validate(&self) -> Result<(), FlightError> {
        if self.pre_trigger > HARD_MAX_PRE_TRIGGER {
            return Err(FlightError::InvalidConfig(format!(
                "pre-trigger duration exceeds {} seconds",
                HARD_MAX_PRE_TRIGGER.as_secs()
            )));
        }
        if self.post_trigger > HARD_MAX_POST_TRIGGER {
            return Err(FlightError::InvalidConfig(format!(
                "post-trigger duration exceeds {} seconds",
                HARD_MAX_POST_TRIGGER.as_secs()
            )));
        }
        if self.max_events == 0 || self.max_events > HARD_MAX_EVENTS {
            return Err(FlightError::InvalidConfig(format!(
                "event bound must be between 1 and {HARD_MAX_EVENTS}"
            )));
        }
        if self.max_bytes == 0 || self.max_bytes > HARD_MAX_BYTES {
            return Err(FlightError::InvalidConfig(format!(
                "byte bound must be between 1 and {HARD_MAX_BYTES}"
            )));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TriggerInfo {
    pub kind: String,
    pub reason: String,
    pub threshold: Option<f64>,
}

impl TriggerInfo {
    pub fn manual() -> Self {
        Self {
            kind: "manual".to_owned(),
            reason: "manual trigger requested".to_owned(),
            threshold: None,
        }
    }

    fn validate(&self) -> Result<(), FlightError> {
        if self.kind.is_empty() || self.kind.len() > 96 {
            return Err(FlightError::InvalidTrigger(
                "trigger kind must contain 1 to 96 bytes".to_owned(),
            ));
        }
        if self.reason.is_empty() || self.reason.len() > 512 {
            return Err(FlightError::InvalidTrigger(
                "trigger reason must contain 1 to 512 bytes".to_owned(),
            ));
        }
        if self.threshold.is_some_and(|value| !value.is_finite()) {
            return Err(FlightError::InvalidTrigger(
                "trigger threshold must be finite".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Termination {
    Normal,
    Interrupted,
    CollectorError,
    RendererError,
}

impl Termination {
    fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Interrupted => "interrupted",
            Self::CollectorError => "collector-error",
            Self::RendererError => "renderer-error",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FlightStatus {
    pub state: FlightState,
    pub retained_events: usize,
    pub retained_bytes: usize,
    pub retained_duration: f64,
    pub pre_trigger_evictions: u64,
    pub tail_elapsed: f64,
    pub tail_duration: f64,
    pub trigger_kind: Option<String>,
    pub output: PathBuf,
}

#[derive(Clone)]
struct BufferedEvent {
    observed_at: f64,
    encoded_bytes: usize,
    event: NormalizedEvent,
}

struct TriggeredState {
    trigger_at: f64,
    trigger_timestamp: Option<f64>,
    trigger: TriggerInfo,
    pre_events: u64,
    actual_pre_duration: f64,
    writer: AtomicIncidentWriter,
    post_events: u64,
    last_event_timestamp: Option<f64>,
}

pub struct FlightRecorder {
    config: FlightConfig,
    state: FlightState,
    armed_at: Option<f64>,
    history: VecDeque<BufferedEvent>,
    history_bytes: usize,
    pre_trigger_evictions: u64,
    triggered: Option<TriggeredState>,
}

impl FlightRecorder {
    pub fn new(config: FlightConfig) -> Result<Self, FlightError> {
        config.validate()?;
        Ok(Self {
            config,
            state: FlightState::Disarmed,
            armed_at: None,
            history: VecDeque::new(),
            history_bytes: 0,
            pre_trigger_evictions: 0,
            triggered: None,
        })
    }

    pub fn state(&self) -> FlightState {
        self.state
    }

    pub fn arm(&mut self, now: f64) -> Result<(), FlightError> {
        self.require_state(FlightState::Disarmed, "arm")?;
        validate_now(now)?;
        if self.config.output.exists() {
            return Err(FlightError::OutputExists(self.config.output.clone()));
        }
        let part = part_path(&self.config.output);
        if part.exists() {
            return Err(FlightError::PartialExists(part));
        }
        self.armed_at = Some(now);
        self.state = FlightState::Armed;
        Ok(())
    }

    pub fn observe(
        &mut self,
        event: NormalizedEvent,
        now: f64,
        losses: IncidentLosses,
    ) -> Result<bool, FlightError> {
        validate_now(now)?;
        match self.state {
            FlightState::Armed => self.retain(event, now),
            FlightState::CapturingTail => {
                if self.tail_expired(now) {
                    return self.finish(now, losses, Termination::Normal).map(|()| true);
                }
                let triggered = self.triggered.as_mut().expect("state invariant");
                triggered.last_event_timestamp = event.timestamp;
                triggered.writer.write_event(event, "post")?;
                triggered.post_events = triggered.post_events.saturating_add(1);
            }
            state => {
                return Err(FlightError::IllegalTransition {
                    state,
                    operation: "observe",
                });
            }
        }
        Ok(false)
    }

    pub fn trigger(
        &mut self,
        now: f64,
        recorded_timestamp: Option<f64>,
        trigger: TriggerInfo,
        losses: IncidentLosses,
    ) -> Result<bool, FlightError> {
        self.require_state(FlightState::Armed, "trigger")?;
        validate_now(now)?;
        trigger.validate()?;
        if recorded_timestamp.is_some_and(|value| !value.is_finite()) {
            return Err(FlightError::InvalidTrigger(
                "recorded trigger timestamp must be finite".to_owned(),
            ));
        }

        let actual_pre_duration = self
            .history
            .front()
            .map_or(0.0, |event| (now - event.observed_at).max(0.0));
        let pre_events = self.history.len() as u64;
        let mut writer = match AtomicIncidentWriter::create(&self.config.output) {
            Ok(writer) => writer,
            Err(error) => {
                self.state = FlightState::Failed;
                return Err(error);
            }
        };
        if let Err(error) = writer.write_start_metadata(
            &self.config,
            self.armed_at.expect("armed state"),
            now,
            recorded_timestamp,
            &trigger,
            actual_pre_duration,
            pre_events,
            self.history_bytes,
            self.pre_trigger_evictions,
            losses,
        ) {
            self.state = FlightState::Failed;
            return Err(error);
        }
        while let Some(buffered) = self.history.pop_front() {
            self.history_bytes = self.history_bytes.saturating_sub(buffered.encoded_bytes);
            if let Err(error) = writer.write_event(buffered.event, "pre") {
                self.state = FlightState::Failed;
                return Err(error);
            }
        }
        if let Err(error) = writer.write_trigger_marker(recorded_timestamp, &trigger) {
            self.state = FlightState::Failed;
            return Err(error);
        }
        self.triggered = Some(TriggeredState {
            trigger_at: now,
            trigger_timestamp: recorded_timestamp,
            trigger,
            pre_events,
            actual_pre_duration,
            writer,
            post_events: 0,
            last_event_timestamp: recorded_timestamp,
        });
        self.state = FlightState::CapturingTail;
        if self.config.post_trigger.is_zero() {
            self.finish(now, losses, Termination::Normal)?;
            return Ok(true);
        }
        Ok(false)
    }

    pub fn tick(&mut self, now: f64, losses: IncidentLosses) -> Result<bool, FlightError> {
        validate_now(now)?;
        if self.state == FlightState::CapturingTail && self.tail_expired(now) {
            self.finish(now, losses, Termination::Normal)?;
            return Ok(true);
        }
        Ok(false)
    }

    pub fn cancel(&mut self) -> Result<(), FlightError> {
        self.require_state(FlightState::Armed, "cancel")?;
        self.history.clear();
        self.history_bytes = 0;
        self.state = FlightState::Cancelled;
        Ok(())
    }

    pub fn interrupt(&mut self, now: f64, losses: IncidentLosses) -> Result<bool, FlightError> {
        match self.state {
            FlightState::Armed => {
                self.cancel()?;
                Ok(false)
            }
            FlightState::CapturingTail => {
                self.finish(now, losses, Termination::Interrupted)?;
                Ok(true)
            }
            state => Err(FlightError::IllegalTransition {
                state,
                operation: "interrupt",
            }),
        }
    }

    pub fn fail_after_trigger(
        &mut self,
        now: f64,
        losses: IncidentLosses,
        termination: Termination,
    ) -> Result<bool, FlightError> {
        if !matches!(
            termination,
            Termination::CollectorError | Termination::RendererError
        ) {
            return Err(FlightError::InvalidConfig(
                "failure completion requires an error termination".to_owned(),
            ));
        }
        match self.state {
            FlightState::Armed => {
                self.cancel()?;
                Ok(false)
            }
            FlightState::CapturingTail => {
                self.finish(now, losses, termination)?;
                Ok(true)
            }
            state => Err(FlightError::IllegalTransition {
                state,
                operation: "fail",
            }),
        }
    }

    pub fn status(&self, now: f64) -> FlightStatus {
        let retained_duration = self
            .history
            .front()
            .map_or(0.0, |event| (now - event.observed_at).max(0.0));
        let (tail_elapsed, trigger_kind) =
            self.triggered.as_ref().map_or((0.0, None), |triggered| {
                (
                    (now - triggered.trigger_at).max(0.0),
                    Some(triggered.trigger.kind.clone()),
                )
            });
        FlightStatus {
            state: self.state,
            retained_events: self.history.len(),
            retained_bytes: self.history_bytes,
            retained_duration,
            pre_trigger_evictions: self.pre_trigger_evictions,
            tail_elapsed,
            tail_duration: self.config.post_trigger.as_secs_f64(),
            trigger_kind,
            output: self.config.output.clone(),
        }
    }

    fn retain(&mut self, event: NormalizedEvent, now: f64) {
        let encoded_bytes = serde_json::to_vec(&event)
            .map(|bytes| bytes.len().saturating_add(1))
            .unwrap_or(HARD_MAX_BYTES);
        self.history.push_back(BufferedEvent {
            observed_at: now,
            encoded_bytes,
            event,
        });
        self.history_bytes = self.history_bytes.saturating_add(encoded_bytes);

        let cutoff = now - self.config.pre_trigger.as_secs_f64();
        while self
            .history
            .front()
            .is_some_and(|event| event.observed_at < cutoff)
        {
            self.pop_oldest(false);
        }
        while self.history.len() > self.config.max_events
            || self.history_bytes > self.config.max_bytes
        {
            self.pop_oldest(true);
        }
    }

    fn pop_oldest(&mut self, capacity_eviction: bool) {
        if let Some(oldest) = self.history.pop_front() {
            self.history_bytes = self.history_bytes.saturating_sub(oldest.encoded_bytes);
            if capacity_eviction {
                self.pre_trigger_evictions = self.pre_trigger_evictions.saturating_add(1);
            }
        }
    }

    fn tail_expired(&self, now: f64) -> bool {
        self.triggered.as_ref().is_some_and(|triggered| {
            now - triggered.trigger_at >= self.config.post_trigger.as_secs_f64()
        })
    }

    fn finish(
        &mut self,
        now: f64,
        losses: IncidentLosses,
        termination: Termination,
    ) -> Result<(), FlightError> {
        self.require_state(FlightState::CapturingTail, "finish")?;
        let triggered = self.triggered.take().expect("state invariant");
        let actual_post_duration = (now - triggered.trigger_at)
            .max(0.0)
            .min(self.config.post_trigger.as_secs_f64());
        let result = triggered.writer.finish(
            &self.config,
            triggered
                .last_event_timestamp
                .or(triggered.trigger_timestamp),
            &triggered.trigger,
            triggered.pre_events,
            triggered.post_events,
            triggered.actual_pre_duration,
            actual_post_duration,
            self.pre_trigger_evictions,
            losses,
            termination,
        );
        match result {
            Ok(()) => {
                self.state = FlightState::Complete;
                Ok(())
            }
            Err(error) => {
                self.state = FlightState::Failed;
                Err(error)
            }
        }
    }

    fn require_state(
        &self,
        required: FlightState,
        operation: &'static str,
    ) -> Result<(), FlightError> {
        if self.state == required {
            Ok(())
        } else {
            Err(FlightError::IllegalTransition {
                state: self.state,
                operation,
            })
        }
    }
}

struct AtomicIncidentWriter {
    output: PathBuf,
    part: PathBuf,
    writer: Option<BufWriter<File>>,
}

impl AtomicIncidentWriter {
    fn create(output: &Path) -> Result<Self, FlightError> {
        if output.exists() {
            return Err(FlightError::OutputExists(output.to_owned()));
        }
        let part = part_path(output);
        if part.exists() {
            return Err(FlightError::PartialExists(part));
        }
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&part)
            .map_err(|source| FlightError::Io {
                action: "create partial incident",
                path: part.clone(),
                source,
            })?;
        Ok(Self {
            output: output.to_owned(),
            part,
            writer: Some(BufWriter::with_capacity(64 * 1024, file)),
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn write_start_metadata(
        &mut self,
        config: &FlightConfig,
        armed_at: f64,
        trigger_at: f64,
        timestamp: Option<f64>,
        trigger: &TriggerInfo,
        actual_pre_duration: f64,
        pre_events: u64,
        pre_bytes: usize,
        pre_evictions: u64,
        losses: IncidentLosses,
    ) -> Result<(), FlightError> {
        let mut labels = base_metadata(config, "start");
        labels.extend([
            ("armed_at".to_owned(), format_f64(armed_at)),
            ("trigger_at".to_owned(), format_f64(trigger_at)),
            ("trigger_kind".to_owned(), trigger.kind.clone()),
            ("trigger_reason".to_owned(), trigger.reason.clone()),
            (
                "actual_pre_trigger_seconds".to_owned(),
                format_f64(actual_pre_duration),
            ),
            ("pre_event_count".to_owned(), pre_events.to_string()),
            ("pre_encoded_bytes".to_owned(), pre_bytes.to_string()),
            (
                "pre_trigger_evictions".to_owned(),
                pre_evictions.to_string(),
            ),
        ]);
        if let Some(threshold) = trigger.threshold {
            labels.insert("trigger_threshold".to_owned(), format_f64(threshold));
        }
        insert_losses(&mut labels, "trigger", losses);
        self.write_record(&metadata_event(timestamp, labels))
    }

    fn write_trigger_marker(
        &mut self,
        timestamp: Option<f64>,
        trigger: &TriggerInfo,
    ) -> Result<(), FlightError> {
        let mut labels = BTreeMap::from([
            (PHASE_LABEL.to_owned(), "trigger".to_owned()),
            ("trigger_kind".to_owned(), trigger.kind.clone()),
            ("trigger_reason".to_owned(), trigger.reason.clone()),
        ]);
        if let Some(threshold) = trigger.threshold {
            labels.insert("trigger_threshold".to_owned(), format_f64(threshold));
        }
        self.write_record(&NormalizedEvent {
            v: SCHEMA_VERSION,
            timestamp,
            category: TRIGGER_CATEGORY.to_owned(),
            origin: None,
            target: None,
            magnitude: 1.0,
            direction: Direction::Neutral,
            labels,
        })
    }

    fn write_event(&mut self, mut event: NormalizedEvent, phase: &str) -> Result<(), FlightError> {
        event
            .labels
            .insert(PHASE_LABEL.to_owned(), phase.to_owned());
        self.write_record(&event)
    }

    #[allow(clippy::too_many_arguments)]
    fn finish(
        mut self,
        config: &FlightConfig,
        timestamp: Option<f64>,
        trigger: &TriggerInfo,
        pre_events: u64,
        post_events: u64,
        actual_pre_duration: f64,
        actual_post_duration: f64,
        pre_evictions: u64,
        losses: IncidentLosses,
        termination: Termination,
    ) -> Result<(), FlightError> {
        let mut labels = base_metadata(config, "end");
        labels.extend([
            ("trigger_kind".to_owned(), trigger.kind.clone()),
            ("trigger_reason".to_owned(), trigger.reason.clone()),
            ("termination".to_owned(), termination.as_str().to_owned()),
            (
                "actual_pre_trigger_seconds".to_owned(),
                format_f64(actual_pre_duration),
            ),
            (
                "actual_post_trigger_seconds".to_owned(),
                format_f64(actual_post_duration),
            ),
            ("pre_event_count".to_owned(), pre_events.to_string()),
            ("post_event_count".to_owned(), post_events.to_string()),
            (
                "pre_trigger_evictions".to_owned(),
                pre_evictions.to_string(),
            ),
        ]);
        if let Some(threshold) = trigger.threshold {
            labels.insert("trigger_threshold".to_owned(), format_f64(threshold));
        }
        insert_losses(&mut labels, "final", losses);
        self.write_record(&metadata_event(timestamp, labels))?;

        let mut writer = self.writer.take().expect("writer invariant");
        writer.flush().map_err(|source| FlightError::Io {
            action: "flush partial incident",
            path: self.part.clone(),
            source,
        })?;
        writer
            .get_ref()
            .sync_all()
            .map_err(|source| FlightError::Io {
                action: "sync partial incident",
                path: self.part.clone(),
                source,
            })?;
        drop(writer);
        fs::rename(&self.part, &self.output).map_err(|source| FlightError::Io {
            action: "publish incident",
            path: self.output.clone(),
            source,
        })
    }

    fn write_record(&mut self, event: &NormalizedEvent) -> Result<(), FlightError> {
        let writer = self.writer.as_mut().expect("writer invariant");
        serde_json::to_writer(&mut *writer, event).map_err(|source| FlightError::Serialize {
            path: self.part.clone(),
            source,
        })?;
        writer.write_all(b"\n").map_err(|source| FlightError::Io {
            action: "write partial incident",
            path: self.part.clone(),
            source,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParsedFlightMetadata {
    pub record: String,
    pub source: FlightSource,
    pub format_version: u16,
}

pub fn parse_metadata_event(
    event: &NormalizedEvent,
) -> Result<Option<ParsedFlightMetadata>, FlightError> {
    if event.category != METADATA_CATEGORY {
        return Ok(None);
    }
    let version = label(event, "format_version")?
        .parse::<u16>()
        .map_err(|_| FlightError::MalformedMetadata("invalid format_version".to_owned()))?;
    if version != FLIGHT_FORMAT_VERSION {
        return Err(FlightError::UnsupportedMetadataVersion(version));
    }
    let source = match label(event, "source")? {
        "scheduler" => FlightSource::Scheduler,
        "tcp" => FlightSource::Tcp,
        _ => return Err(FlightError::MalformedMetadata("invalid source".to_owned())),
    };
    Ok(Some(ParsedFlightMetadata {
        record: label(event, "record")?.to_owned(),
        source,
        format_version: version,
    }))
}

fn label<'a>(event: &'a NormalizedEvent, key: &str) -> Result<&'a str, FlightError> {
    event
        .labels
        .get(key)
        .map(String::as_str)
        .ok_or_else(|| FlightError::MalformedMetadata(format!("missing {key}")))
}

fn metadata_event(timestamp: Option<f64>, labels: BTreeMap<String, String>) -> NormalizedEvent {
    NormalizedEvent {
        v: SCHEMA_VERSION,
        timestamp,
        category: METADATA_CATEGORY.to_owned(),
        origin: None,
        target: None,
        magnitude: 0.0,
        direction: Direction::Neutral,
        labels,
    }
}

fn base_metadata(config: &FlightConfig, record: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            "format_version".to_owned(),
            FLIGHT_FORMAT_VERSION.to_string(),
        ),
        ("record".to_owned(), record.to_owned()),
        ("source".to_owned(), config.source.as_str().to_owned()),
        (
            "configured_pre_trigger_seconds".to_owned(),
            format_f64(config.pre_trigger.as_secs_f64()),
        ),
        (
            "configured_post_trigger_seconds".to_owned(),
            format_f64(config.post_trigger.as_secs_f64()),
        ),
        ("timestamp_domain".to_owned(), "kernel-monotonic".to_owned()),
        ("kernel".to_owned(), config.host.kernel.clone()),
        ("architecture".to_owned(), config.host.architecture.clone()),
    ])
}

fn insert_losses(labels: &mut BTreeMap<String, String>, prefix: &str, losses: IncidentLosses) {
    for (name, value) in [
        ("kernel_ring_loss", losses.kernel_ring),
        ("collector_loss", losses.collector),
        ("ipc_loss", losses.ipc),
        ("renderer_channel_loss", losses.renderer_channel),
        ("malformed_count", losses.malformed),
        ("writer_loss", losses.writer),
    ] {
        labels.insert(format!("{prefix}_{name}"), value.to_string());
    }
}

fn format_f64(value: f64) -> String {
    format!("{value:.6}")
}

fn validate_now(now: f64) -> Result<(), FlightError> {
    if now.is_finite() && now >= 0.0 {
        Ok(())
    } else {
        Err(FlightError::InvalidConfig(
            "monotonic time must be finite and non-negative".to_owned(),
        ))
    }
}

pub fn part_path(output: &Path) -> PathBuf {
    let mut value: OsString = output.as_os_str().to_owned();
    value.push(".part");
    PathBuf::from(value)
}

#[derive(Debug, Error)]
pub enum FlightError {
    #[error("invalid flight-recorder configuration: {0}")]
    InvalidConfig(String),
    #[error("invalid flight-recorder trigger: {0}")]
    InvalidTrigger(String),
    #[error("cannot {operation} flight recorder while it is {state:?}")]
    IllegalTransition {
        state: FlightState,
        operation: &'static str,
    },
    #[error("refusing to overwrite existing incident {}", .0.display())]
    OutputExists(PathBuf),
    #[error("refusing to overwrite existing partial incident {}", .0.display())]
    PartialExists(PathBuf),
    #[error("unsupported flight metadata version {0}; supported version is 1")]
    UnsupportedMetadataVersion(u16),
    #[error("malformed flight metadata: {0}")]
    MalformedMetadata(String),
    #[error("could not {action} {}: {source}", path.display())]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not serialize incident record to {}: {source}", path.display())]
    Serialize {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use crate::source::demo::DemoSource;

    use super::*;

    fn temporary_path(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("synesthesia-flight-{label}-{unique}.ndjson"))
    }

    fn config(path: PathBuf) -> FlightConfig {
        FlightConfig {
            output: path,
            source: FlightSource::Tcp,
            pre_trigger: Duration::from_secs(2),
            post_trigger: Duration::from_secs(1),
            max_events: 32,
            max_bytes: 64 * 1024,
            host: HostMetadata {
                kernel: "test-kernel".to_owned(),
                architecture: "test-arch".to_owned(),
            },
        }
    }

    fn event(index: usize) -> NormalizedEvent {
        let mut event = DemoSource::new(42).nth(index).unwrap();
        event.timestamp = Some(index as f64 * 0.1);
        event
    }

    fn cleanup(path: &Path) {
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(part_path(path));
    }

    #[test]
    fn explicit_state_machine_refuses_illegal_transitions() {
        let path = temporary_path("states");
        let mut recorder = FlightRecorder::new(config(path.clone())).unwrap();
        assert_eq!(recorder.state(), FlightState::Disarmed);
        assert!(matches!(
            recorder.trigger(0.0, None, TriggerInfo::manual(), IncidentLosses::default()),
            Err(FlightError::IllegalTransition { .. })
        ));
        recorder.arm(0.0).unwrap();
        assert!(matches!(
            recorder.arm(0.1),
            Err(FlightError::IllegalTransition { .. })
        ));
        recorder.cancel().unwrap();
        assert_eq!(recorder.state(), FlightState::Cancelled);
        cleanup(&path);
    }

    #[test]
    fn manual_trigger_preserves_order_and_completes_after_bounded_tail() {
        let path = temporary_path("manual");
        let mut recorder = FlightRecorder::new(config(path.clone())).unwrap();
        recorder.arm(0.0).unwrap();
        recorder
            .observe(event(1), 0.2, IncidentLosses::default())
            .unwrap();
        recorder
            .observe(event(2), 0.4, IncidentLosses::default())
            .unwrap();
        assert!(
            !recorder
                .trigger(
                    0.5,
                    Some(0.5),
                    TriggerInfo::manual(),
                    IncidentLosses::default()
                )
                .unwrap()
        );
        recorder
            .observe(event(3), 0.8, IncidentLosses::default())
            .unwrap();
        assert!(recorder.tick(1.5, IncidentLosses::default()).unwrap());
        assert_eq!(recorder.state(), FlightState::Complete);
        assert!(path.exists());
        assert!(!part_path(&path).exists());

        let records: Vec<NormalizedEvent> = fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(records[0].category, METADATA_CATEGORY);
        assert_eq!(records[1].labels[PHASE_LABEL], "pre");
        assert_eq!(records[2].labels[PHASE_LABEL], "pre");
        assert_eq!(records[3].category, TRIGGER_CATEGORY);
        assert_eq!(records[4].labels[PHASE_LABEL], "post");
        assert_eq!(records[5].labels["record"], "end");
        cleanup(&path);
    }

    #[test]
    fn time_event_and_byte_bounds_evict_oldest_deterministically() {
        let path = temporary_path("bounds");
        let mut bounded = config(path.clone());
        bounded.pre_trigger = Duration::from_millis(500);
        bounded.max_events = 2;
        bounded.max_bytes = 900;
        let mut recorder = FlightRecorder::new(bounded).unwrap();
        recorder.arm(0.0).unwrap();
        for index in 0..5 {
            recorder
                .observe(event(index), index as f64 * 0.2, IncidentLosses::default())
                .unwrap();
        }
        let status = recorder.status(0.8);
        assert!(status.retained_events <= 2);
        assert!(status.retained_bytes <= 900);
        assert!(status.retained_duration <= 0.5);
        assert!(status.pre_trigger_evictions > 0);
        recorder.cancel().unwrap();
        cleanup(&path);
    }

    #[test]
    fn cancellation_creates_no_file_and_existing_paths_are_refused() {
        let path = temporary_path("cancel");
        let mut recorder = FlightRecorder::new(config(path.clone())).unwrap();
        recorder.arm(0.0).unwrap();
        recorder
            .observe(event(0), 0.1, IncidentLosses::default())
            .unwrap();
        recorder.cancel().unwrap();
        assert!(!path.exists());
        assert!(!part_path(&path).exists());

        fs::write(&path, b"existing").unwrap();
        let mut recorder = FlightRecorder::new(config(path.clone())).unwrap();
        assert!(matches!(
            recorder.arm(0.0),
            Err(FlightError::OutputExists(_))
        ));
        assert_eq!(fs::read(&path).unwrap(), b"existing");
        cleanup(&path);
    }

    #[test]
    fn interrupted_tail_publishes_valid_early_incident() {
        let path = temporary_path("interrupt");
        let mut recorder = FlightRecorder::new(config(path.clone())).unwrap();
        recorder.arm(0.0).unwrap();
        recorder
            .observe(event(0), 0.1, IncidentLosses::default())
            .unwrap();
        recorder
            .trigger(
                0.2,
                Some(0.2),
                TriggerInfo::manual(),
                IncidentLosses::default(),
            )
            .unwrap();
        assert!(recorder.interrupt(0.4, IncidentLosses::default()).unwrap());
        let final_record: NormalizedEvent =
            serde_json::from_str(fs::read_to_string(&path).unwrap().lines().last().unwrap())
                .unwrap();
        assert_eq!(final_record.labels["termination"], "interrupted");
        cleanup(&path);
    }

    #[test]
    fn writer_failure_never_presents_a_final_incident() {
        let parent = temporary_path("missing-parent");
        let path = parent.join("incident.ndjson");
        let mut recorder = FlightRecorder::new(config(path.clone())).unwrap();
        recorder.arm(0.0).unwrap();
        assert!(
            recorder
                .trigger(
                    0.1,
                    Some(0.1),
                    TriggerInfo::manual(),
                    IncidentLosses::default()
                )
                .is_err()
        );
        assert_eq!(recorder.state(), FlightState::Failed);
        assert!(!path.exists());
    }

    #[test]
    fn metadata_version_is_explicit_and_refused_when_unknown() {
        let mut event = metadata_event(
            Some(1.0),
            BTreeMap::from([
                ("format_version".to_owned(), "1".to_owned()),
                ("record".to_owned(), "start".to_owned()),
                ("source".to_owned(), "tcp".to_owned()),
            ]),
        );
        assert_eq!(
            parse_metadata_event(&event)
                .unwrap()
                .unwrap()
                .format_version,
            1
        );
        event
            .labels
            .insert("format_version".to_owned(), "9".to_owned());
        assert!(matches!(
            parse_metadata_event(&event),
            Err(FlightError::UnsupportedMetadataVersion(9))
        ));
    }

    #[test]
    fn deterministic_inputs_produce_deterministic_incident_bytes() {
        let left = temporary_path("deterministic-left");
        let right = temporary_path("deterministic-right");
        for path in [&left, &right] {
            let mut recorder = FlightRecorder::new(config(path.clone())).unwrap();
            recorder.arm(0.0).unwrap();
            recorder
                .observe(event(0), 0.1, IncidentLosses::default())
                .unwrap();
            recorder
                .trigger(
                    0.2,
                    Some(0.2),
                    TriggerInfo::manual(),
                    IncidentLosses::default(),
                )
                .unwrap();
            recorder.tick(1.2, IncidentLosses::default()).unwrap();
        }
        assert_eq!(fs::read(&left).unwrap(), fs::read(&right).unwrap());
        cleanup(&left);
        cleanup(&right);
    }
}
