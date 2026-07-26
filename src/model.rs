use std::collections::{HashMap, VecDeque};

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
        let lane_material = event
            .origin
            .as_deref()
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
            category: stable_hash(event.category.as_bytes()),
            magnitude: event.magnitude,
            direction: event.direction,
        });
        if self.rates.len() == MAX_ACTIVITY {
            self.rates.pop_front();
        }
        self.rates.push_back(RateSample {
            time: self.now,
            magnitude: event.magnitude,
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
        ModelSnapshot {
            now: self.now,
            decay_seconds: self.decay_seconds,
            activity: self.activity.iter().cloned().collect(),
            metrics: Metrics {
                events_per_second: self.rates.len() as f64 / elapsed,
                magnitude_per_second: self
                    .rates
                    .iter()
                    .map(|sample| sample.magnitude)
                    .sum::<f64>()
                    / elapsed,
                active_flows: self.flows.len(),
            },
        }
    }
}

impl Default for TemporalModel {
    fn default() -> Self {
        Self::new(2.8)
    }
}

#[cfg(test)]
mod tests {
    use crate::source::demo::DemoSource;

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
}
