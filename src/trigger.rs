use std::{collections::VecDeque, str::FromStr};

use thiserror::Error;

use crate::{
    event::NormalizedEvent,
    flight_recorder::{FlightSource, TriggerInfo},
};

const WINDOW_SECONDS: f64 = 0.25;
const DEBOUNCE_WINDOWS: usize = 3;
const REQUIRED_EXCEEDED_WINDOWS: usize = 2;

#[derive(Clone, Debug, PartialEq)]
pub enum TriggerSpec {
    Manual,
    Auto,
    TcpRetransmitRate(f64),
    TcpReset,
    SchedulerEventRate(f64),
    SchedulerMigrationRate(f64),
}

impl TriggerSpec {
    pub fn resolve(self, source: FlightSource) -> Self {
        match (self, source) {
            (Self::Auto, FlightSource::Tcp) => Self::TcpRetransmitRate(100.0),
            (Self::Auto, FlightSource::Scheduler) => Self::SchedulerEventRate(15_000.0),
            (other, _) => other,
        }
    }

    pub fn validate_for(&self, source: FlightSource) -> Result<(), TriggerError> {
        let compatible = matches!(
            (source, self),
            (_, Self::Manual | Self::Auto)
                | (
                    FlightSource::Tcp,
                    Self::TcpRetransmitRate(_) | Self::TcpReset
                )
                | (
                    FlightSource::Scheduler,
                    Self::SchedulerEventRate(_) | Self::SchedulerMigrationRate(_)
                )
        );
        if !compatible {
            return Err(TriggerError::WrongSource {
                source_kind: source,
                trigger: self.to_string(),
            });
        }
        Ok(())
    }

    fn threshold(&self) -> Option<f64> {
        match self {
            Self::TcpRetransmitRate(value)
            | Self::SchedulerEventRate(value)
            | Self::SchedulerMigrationRate(value) => Some(*value),
            Self::Manual | Self::Auto | Self::TcpReset => None,
        }
    }

    fn rate_kind(&self) -> Option<RateKind> {
        match self {
            Self::TcpRetransmitRate(_) => Some(RateKind::TcpRetransmit),
            Self::SchedulerEventRate(_) => Some(RateKind::SchedulerEvent),
            Self::SchedulerMigrationRate(_) => Some(RateKind::SchedulerMigration),
            Self::Manual | Self::Auto | Self::TcpReset => None,
        }
    }
}

impl std::fmt::Display for TriggerSpec {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Manual => write!(formatter, "manual"),
            Self::Auto => write!(formatter, "auto"),
            Self::TcpRetransmitRate(value) => {
                write!(formatter, "tcp-retransmit-rate={}", concise(*value))
            }
            Self::TcpReset => write!(formatter, "tcp-reset"),
            Self::SchedulerEventRate(value) => {
                write!(formatter, "scheduler-event-rate={}", concise(*value))
            }
            Self::SchedulerMigrationRate(value) => {
                write!(formatter, "scheduler-migration-rate={}", concise(*value))
            }
        }
    }
}

impl FromStr for TriggerSpec {
    type Err = TriggerError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "manual" => Ok(Self::Manual),
            "auto" => Ok(Self::Auto),
            "tcp-reset" => Ok(Self::TcpReset),
            _ => {
                for (prefix, constructor) in [
                    (
                        "tcp-retransmit-rate=",
                        Self::TcpRetransmitRate as fn(f64) -> Self,
                    ),
                    ("scheduler-event-rate=", Self::SchedulerEventRate),
                    ("scheduler-migration-rate=", Self::SchedulerMigrationRate),
                ] {
                    if let Some(threshold) = value.strip_prefix(prefix) {
                        let threshold = threshold.parse::<f64>().map_err(|_| {
                            TriggerError::Invalid(format!("{prefix} requires a number"))
                        })?;
                        if threshold.is_finite() && threshold > 0.0 {
                            return Ok(constructor(threshold));
                        }
                        return Err(TriggerError::Invalid(
                            "trigger threshold must be finite and greater than zero".to_owned(),
                        ));
                    }
                }
                Err(TriggerError::Invalid(format!(
                    "unknown trigger {value}; expected manual, auto, tcp-reset, or a typed rate threshold"
                )))
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RateKind {
    TcpRetransmit,
    SchedulerEvent,
    SchedulerMigration,
}

pub struct TriggerEvaluator {
    spec: TriggerSpec,
    current_start: Option<f64>,
    current_count: f64,
    completed: VecDeque<bool>,
    fired: bool,
}

impl TriggerEvaluator {
    pub fn new(spec: TriggerSpec, source: FlightSource) -> Result<Self, TriggerError> {
        spec.validate_for(source)?;
        let spec = spec.resolve(source);
        Ok(Self {
            spec,
            current_start: None,
            current_count: 0.0,
            completed: VecDeque::with_capacity(DEBOUNCE_WINDOWS),
            fired: false,
        })
    }

    pub fn spec(&self) -> &TriggerSpec {
        &self.spec
    }

    pub fn observe(&mut self, event: &NormalizedEvent, now: f64) -> Option<TriggerInfo> {
        if self.fired || !now.is_finite() || now < 0.0 {
            return None;
        }
        if self.spec == TriggerSpec::TcpReset
            && matches!(
                event.category.as_str(),
                "tcp.reset.send" | "tcp.reset.receive"
            )
        {
            self.fired = true;
            return Some(TriggerInfo {
                kind: "tcp-reset".to_owned(),
                reason: format!("observed semantic {}", event.category),
                threshold: None,
            });
        }
        let kind = self.spec.rate_kind()?;
        let threshold = self.spec.threshold().expect("rate trigger");
        let start = *self.current_start.get_or_insert(now);
        let mut window_start = start;
        while now - window_start >= WINDOW_SECONDS {
            self.close_window(threshold);
            window_start += WINDOW_SECONDS;
            self.current_start = Some(window_start);
            if self.debounce_satisfied() {
                self.fired = true;
                return Some(rate_trigger(kind, threshold));
            }
        }
        if rate_kind_matches(kind, event) {
            self.current_count += semantic_count(event);
        }
        None
    }

    pub fn tick(&mut self, now: f64) -> Option<TriggerInfo> {
        if self.fired {
            return None;
        }
        let kind = self.spec.rate_kind()?;
        let threshold = self.spec.threshold().expect("rate trigger");
        let Some(mut start) = self.current_start else {
            self.current_start = Some(now);
            return None;
        };
        while now - start >= WINDOW_SECONDS {
            self.close_window(threshold);
            start += WINDOW_SECONDS;
            self.current_start = Some(start);
            if self.debounce_satisfied() {
                self.fired = true;
                return Some(rate_trigger(kind, threshold));
            }
        }
        None
    }

    pub fn manual(&mut self) -> Option<TriggerInfo> {
        if self.fired {
            return None;
        }
        self.fired = true;
        Some(TriggerInfo::manual())
    }

    fn close_window(&mut self, threshold: f64) {
        let rate = self.current_count / WINDOW_SECONDS;
        if self.completed.len() == DEBOUNCE_WINDOWS {
            self.completed.pop_front();
        }
        self.completed.push_back(rate >= threshold);
        self.current_count = 0.0;
    }

    fn debounce_satisfied(&self) -> bool {
        self.completed.len() == DEBOUNCE_WINDOWS
            && self.completed.iter().filter(|exceeded| **exceeded).count()
                >= REQUIRED_EXCEEDED_WINDOWS
    }
}

fn rate_kind_matches(kind: RateKind, event: &NormalizedEvent) -> bool {
    match kind {
        RateKind::TcpRetransmit => event.category == "tcp.retransmit",
        RateKind::SchedulerEvent => event.category.starts_with("sched."),
        RateKind::SchedulerMigration => event.category == "sched.migrate",
    }
}

fn semantic_count(event: &NormalizedEvent) -> f64 {
    event
        .labels
        .get("synesthesia.event_count")
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value >= 0.0)
        .unwrap_or(1.0)
}

fn rate_trigger(kind: RateKind, threshold: f64) -> TriggerInfo {
    let (name, observed) = match kind {
        RateKind::TcpRetransmit => ("tcp-retransmit-rate", "TCP retransmit semantic-event"),
        RateKind::SchedulerEvent => ("scheduler-event-rate", "scheduler semantic-event"),
        RateKind::SchedulerMigration => (
            "scheduler-migration-rate",
            "scheduler migration semantic-event",
        ),
    };
    TriggerInfo {
        kind: name.to_owned(),
        reason: format!(
            "{observed} rate exceeded {}/s in at least 2 of 3 250 ms windows",
            concise(threshold)
        ),
        threshold: Some(threshold),
    }
}

fn concise(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        value.to_string()
    }
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum TriggerError {
    #[error("invalid flight trigger: {0}")]
    Invalid(String),
    #[error("trigger {trigger} is incompatible with {source_kind:?} source")]
    WrongSource {
        source_kind: FlightSource,
        trigger: String,
    },
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::event::{Direction, SCHEMA_VERSION};

    use super::*;

    fn event(category: &str, count: u32) -> NormalizedEvent {
        NormalizedEvent {
            v: SCHEMA_VERSION,
            timestamp: Some(1.0),
            category: category.to_owned(),
            origin: None,
            target: None,
            magnitude: 1.0,
            direction: Direction::Neutral,
            labels: BTreeMap::from([("synesthesia.event_count".to_owned(), count.to_string())]),
        }
    }

    #[test]
    fn typed_trigger_parser_refuses_generic_or_cross_source_rules() {
        assert_eq!(
            "tcp-retransmit-rate=100".parse::<TriggerSpec>().unwrap(),
            TriggerSpec::TcpRetransmitRate(100.0)
        );
        assert!("anything>5".parse::<TriggerSpec>().is_err());
        assert!(
            TriggerSpec::TcpReset
                .validate_for(FlightSource::Scheduler)
                .is_err()
        );
    }

    #[test]
    fn manual_trigger_is_one_shot() {
        let mut evaluator = TriggerEvaluator::new(TriggerSpec::Manual, FlightSource::Tcp).unwrap();
        assert_eq!(evaluator.manual().unwrap().kind, "manual");
        assert!(evaluator.manual().is_none());
    }

    #[test]
    fn tcp_reset_fires_immediately_and_once() {
        let mut evaluator =
            TriggerEvaluator::new(TriggerSpec::TcpReset, FlightSource::Tcp).unwrap();
        assert!(
            evaluator
                .observe(&event("tcp.retransmit", 500), 0.0)
                .is_none()
        );
        assert!(
            evaluator
                .observe(&event("tcp.reset.receive", 1), 0.1)
                .is_some()
        );
        assert!(
            evaluator
                .observe(&event("tcp.reset.send", 1), 0.2)
                .is_none()
        );
    }

    #[test]
    fn retransmit_rate_requires_two_of_three_completed_windows() {
        let mut evaluator =
            TriggerEvaluator::new(TriggerSpec::TcpRetransmitRate(100.0), FlightSource::Tcp)
                .unwrap();
        evaluator.observe(&event("tcp.retransmit", 30), 0.0);
        evaluator.tick(0.25);
        evaluator.observe(&event("tcp.retransmit", 1), 0.26);
        evaluator.tick(0.50);
        assert!(
            evaluator
                .observe(&event("tcp.retransmit", 30), 0.51)
                .is_none()
        );
        assert!(evaluator.tick(0.75).is_some());
    }

    #[test]
    fn scheduler_event_and_migration_rates_count_semantic_multiplicity() {
        for (spec, category) in [
            (TriggerSpec::SchedulerEventRate(100.0), "sched.switch"),
            (TriggerSpec::SchedulerMigrationRate(100.0), "sched.migrate"),
        ] {
            let mut evaluator = TriggerEvaluator::new(spec, FlightSource::Scheduler).unwrap();
            for window in 0..3 {
                evaluator.observe(&event(category, 30), window as f64 * 0.25);
            }
            assert!(evaluator.tick(0.75).is_some());
        }
    }

    #[test]
    fn auto_defaults_are_source_specific_and_conservative() {
        assert_eq!(
            TriggerSpec::Auto.resolve(FlightSource::Tcp),
            TriggerSpec::TcpRetransmitRate(100.0)
        );
        assert_eq!(
            TriggerSpec::Auto.resolve(FlightSource::Scheduler),
            TriggerSpec::SchedulerEventRate(15_000.0)
        );
    }
}
