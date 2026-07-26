use std::collections::{HashMap, HashSet, VecDeque};

use crate::{
    event::{Direction, NormalizedEvent},
    source::stable_hash,
};

const MAX_ACTIVITY: usize = 4_096;
const MAX_FLOWS: usize = 512;
const RATE_WINDOW_SECONDS: f64 = 1.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Activity {
    pub born: f64,
    pub lane: u64,
    pub flow: u64,
    pub category: u64,
    pub magnitude: f64,
    pub direction: Direction,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Metrics {
    pub events_per_second: f64,
    pub magnitude_per_second: f64,
    pub active_flows: usize,
    pub scheduler: Option<SchedulerMetrics>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SchedulerMetrics {
    pub switches_per_second: f64,
    pub wakeups_per_second: f64,
    pub migrations_per_second: f64,
    pub active_cpus: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModelSnapshot {
    pub now: f64,
    pub decay_seconds: f64,
    pub activity: Vec<Activity>,
    pub metrics: Metrics,
}

#[derive(Clone, Debug)]
struct RateSample {
    time: f64,
    magnitude: f64,
    count: f64,
    category: u64,
}

pub struct TemporalModel {
    now: f64,
    decay_seconds: f64,
    activity: VecDeque<Activity>,
    rates: VecDeque<RateSample>,
    flows: HashMap<u64, f64>,
}

impl TemporalModel {
    pub fn new(decay_seconds: f64) -> Self {
        Self {
            now: 0.0,
            decay_seconds: decay_seconds.clamp(0.2, 30.0),
            activity: VecDeque::with_capacity(MAX_ACTIVITY),
            rates: VecDeque::with_capacity(256),
            flows: HashMap::with_capacity(MAX_FLOWS),
        }
    }

    pub fn ingest(&mut self, event: NormalizedEvent, now: f64) {
        self.advance(now);
        let flow = stable_hash(event.flow_key().as_bytes());
        let category = stable_hash(event.category.as_bytes());
        let event_count = event
            .labels
            .get("synesthesia.event_count")
            .and_then(|count| count.parse::<f64>().ok())
            .filter(|count| count.is_finite() && *count >= 0.0)
            .unwrap_or(1.0);
        let lane_material = event
            .labels
            .get("synesthesia.lane")
            .map(String::as_str)
            .or(event.origin.as_deref())
            .or(event.target.as_deref())
            .unwrap_or(&event.category);
        let lane = stable_hash(lane_material.as_bytes());
        if self.activity.len() == MAX_ACTIVITY {
            self.activity.pop_front();
        }
        self.activity.push_back(Activity {
            born: self.now,
            lane,
            flow,
            category,
            magnitude: event.magnitude,
            direction: event.direction,
        });
        if self.rates.len() == MAX_ACTIVITY {
            self.rates.pop_front();
        }
        self.rates.push_back(RateSample {
            time: self.now,
            magnitude: event.magnitude,
            count: event_count,
            category,
        });
        if self.flows.len() < MAX_FLOWS || self.flows.contains_key(&flow) {
            self.flows.insert(flow, self.now);
        } else if let Some(oldest) = self
            .flows
            .iter()
            .min_by(|left, right| left.1.total_cmp(right.1))
            .map(|(key, _)| *key)
        {
            self.flows.remove(&oldest);
            self.flows.insert(flow, self.now);
        }
    }

    pub fn advance(&mut self, now: f64) {
        self.now = now.max(self.now);
        let activity_cutoff = self.now - self.decay_seconds * 2.5;
        while self
            .activity
            .front()
            .is_some_and(|activity| activity.born < activity_cutoff)
        {
            self.activity.pop_front();
        }
        let rate_cutoff = self.now - RATE_WINDOW_SECONDS;
        while self
            .rates
            .front()
            .is_some_and(|sample| sample.time < rate_cutoff)
        {
            self.rates.pop_front();
        }
        let flow_cutoff = self.now - self.decay_seconds;
        self.flows.retain(|_, seen| *seen >= flow_cutoff);
    }

    pub fn set_decay(&mut self, seconds: f64) {
        self.decay_seconds = seconds.clamp(0.2, 30.0);
    }

    pub fn snapshot(&self) -> ModelSnapshot {
        let elapsed = self.rates.front().map_or(RATE_WINDOW_SECONDS, |sample| {
            (self.now - sample.time).clamp(0.1, RATE_WINDOW_SECONDS)
        });
        let scheduler = self.scheduler_metrics(elapsed);
        ModelSnapshot {
            now: self.now,
            decay_seconds: self.decay_seconds,
            activity: self.activity.iter().cloned().collect(),
            metrics: Metrics {
                events_per_second: self.rates.iter().map(|sample| sample.count).sum::<f64>()
                    / elapsed,
                magnitude_per_second: self
                    .rates
                    .iter()
                    .map(|sample| sample.magnitude)
                    .sum::<f64>()
                    / elapsed,
                active_flows: self.flows.len(),
                scheduler,
            },
        }
    }

    fn scheduler_metrics(&self, elapsed: f64) -> Option<SchedulerMetrics> {
        let switch = stable_hash(b"sched.switch");
        let wakeup = stable_hash(b"sched.wakeup");
        let migrate = stable_hash(b"sched.migrate");
        if !self
            .rates
            .iter()
            .any(|sample| [switch, wakeup, migrate].contains(&sample.category))
        {
            return None;
        }
        let rate = |category| {
            self.rates
                .iter()
                .filter(|sample| sample.category == category)
                .map(|sample| sample.count)
                .sum::<f64>()
                / elapsed
        };
        let active_cpus = self
            .activity
            .iter()
            .filter(|activity| [switch, wakeup, migrate].contains(&activity.category))
            .map(|activity| activity.lane)
            .collect::<HashSet<_>>()
            .len();
        Some(SchedulerMetrics {
            switches_per_second: rate(switch),
            wakeups_per_second: rate(wakeup),
            migrations_per_second: rate(migrate),
            active_cpus,
        })
    }
}

impl Default for TemporalModel {
    fn default() -> Self {
        Self::new(2.8)
    }
}

#[cfg(test)]
mod tests {
    use crate::{event::NormalizedEvent, source::demo::DemoSource};

    use super::*;

    #[test]
    fn fixed_timestamps_produce_deterministic_evolution() {
        let events: Vec<_> = DemoSource::new(42).take(200).collect();
        let evolve = || {
            let mut model = TemporalModel::default();
            for (index, event) in events.iter().cloned().enumerate() {
                model.ingest(event, index as f64 * 0.025);
            }
            model.advance(5.0);
            model.snapshot()
        };
        assert_eq!(evolve(), evolve());
    }

    #[test]
    fn decay_removes_stale_activity_and_flows() {
        let mut model = TemporalModel::new(1.0);
        model.ingest(DemoSource::new(2).next().unwrap(), 0.0);
        assert_eq!(model.snapshot().activity.len(), 1);
        model.advance(3.0);
        let snapshot = model.snapshot();
        assert!(snapshot.activity.is_empty());
        assert_eq!(snapshot.metrics.active_flows, 0);
        assert_eq!(snapshot.metrics.events_per_second, 0.0);
    }

    #[test]
    fn magnitude_contributes_to_rate_not_just_event_count() {
        let mut events = DemoSource::new(3);
        let mut low = events.next().unwrap();
        low.magnitude = 10.0;
        let mut high = events.next().unwrap();
        high.magnitude = 1_000.0;
        let mut model = TemporalModel::default();
        model.ingest(low, 1.0);
        model.ingest(high, 1.5);
        let metrics = model.snapshot().metrics;
        assert_eq!(metrics.events_per_second, 4.0);
        assert_eq!(metrics.magnitude_per_second, 2_020.0);
    }

    #[test]
    fn aggregated_scheduler_counts_drive_rates_without_particle_sprawl() {
        let mut event: NormalizedEvent = serde_json::from_str(
            include_str!("../examples/scheduler.ndjson")
                .lines()
                .nth(12)
                .unwrap(),
        )
        .unwrap();
        event
            .labels
            .insert("synesthesia.event_count".to_owned(), "64".to_owned());
        let mut model = TemporalModel::default();
        model.ingest(event, 1.0);
        let snapshot = model.snapshot();
        assert_eq!(snapshot.activity.len(), 1);
        assert_eq!(snapshot.metrics.events_per_second, 640.0);
        assert_eq!(
            snapshot.metrics.scheduler.unwrap().switches_per_second,
            640.0
        );
    }

    #[test]
    fn stale_scheduler_activity_loses_active_cpu_state() {
        let event: NormalizedEvent = serde_json::from_str(
            include_str!("../examples/scheduler.ndjson")
                .lines()
                .next()
                .unwrap(),
        )
        .unwrap();
        let mut model = TemporalModel::new(0.5);
        model.ingest(event, 0.0);
        assert_eq!(model.snapshot().metrics.scheduler.unwrap().active_cpus, 1);
        model.advance(2.0);
        assert!(model.snapshot().metrics.scheduler.is_none());
    }
}
