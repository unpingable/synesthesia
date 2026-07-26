use std::collections::{HashMap, HashSet, VecDeque};

use crate::{
    event::{Direction, NormalizedEvent},
    flight_recorder::{IncidentLosses, METADATA_CATEGORY, PHASE_LABEL, TRIGGER_CATEGORY},
    source::stable_hash,
};

const MAX_ACTIVITY: usize = 4_096;
const MAX_FLOWS: usize = 512;
pub const MAX_PARTICLES: usize = 2_048;
const MAX_PARTICLES_PER_EVENT: usize = 16;
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParticleStyle {
    Ember,
    Fracture,
    Impact,
    Migration,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Particle {
    pub born: f64,
    pub lifetime: f64,
    pub origin_x: f64,
    pub origin_y: f64,
    pub velocity_x: f64,
    pub velocity_y: f64,
    pub energy: f32,
    pub category: u64,
    pub direction: Direction,
    pub style: ParticleStyle,
    pub seed: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Metrics {
    pub events_per_second: f64,
    pub magnitude_per_second: f64,
    pub active_flows: usize,
    pub scheduler: Option<SchedulerMetrics>,
    pub tcp: Option<TcpMetrics>,
    pub flight: Option<FlightReplayMetrics>,
    pub particle_evictions: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FlightReplayMetrics {
    pub phase: String,
    pub source: Option<String>,
    pub trigger_kind: Option<String>,
    pub trigger_reason: Option<String>,
    pub losses: IncidentLosses,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SchedulerMetrics {
    pub switches_per_second: f64,
    pub wakeups_per_second: f64,
    pub migrations_per_second: f64,
    pub active_cpus: usize,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TcpMetrics {
    pub retransmits_per_second: f64,
    pub resets_sent_per_second: f64,
    pub resets_received_per_second: f64,
    pub active_pathological_flows: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModelSnapshot {
    pub now: f64,
    pub decay_seconds: f64,
    pub activity: Vec<Activity>,
    pub particles: Vec<Particle>,
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
    particles: VecDeque<Particle>,
    particle_evictions: u64,
    rates: VecDeque<RateSample>,
    flows: HashMap<u64, f64>,
    flight: Option<FlightReplayMetrics>,
}

impl TemporalModel {
    pub fn new(decay_seconds: f64) -> Self {
        Self {
            now: 0.0,
            decay_seconds: decay_seconds.clamp(0.2, 30.0),
            activity: VecDeque::with_capacity(MAX_ACTIVITY),
            particles: VecDeque::with_capacity(MAX_PARTICLES),
            particle_evictions: 0,
            rates: VecDeque::with_capacity(256),
            flows: HashMap::with_capacity(MAX_FLOWS),
            flight: None,
        }
    }

    pub fn ingest(&mut self, event: NormalizedEvent, now: f64) {
        self.advance(now);
        if event.category == METADATA_CATEGORY {
            self.ingest_flight_metadata(&event);
            return;
        }
        if let Some(phase) = event.labels.get(PHASE_LABEL) {
            let flight = self.flight.get_or_insert_with(FlightReplayMetrics::default);
            flight.phase = phase.chars().take(16).collect();
            if let Some(source) = event.labels.get("source") {
                flight.source = Some(source.chars().take(16).collect());
            } else if event.category.starts_with("tcp.") {
                flight.source = Some("tcp".to_owned());
            } else if event.category.starts_with("sched.") {
                flight.source = Some("scheduler".to_owned());
            }
            if event.category == TRIGGER_CATEGORY {
                flight.trigger_kind = event
                    .labels
                    .get("trigger_kind")
                    .map(|value| value.chars().take(96).collect());
                flight.trigger_reason = event
                    .labels
                    .get("trigger_reason")
                    .map(|value| value.chars().take(512).collect());
            }
        }
        if event.category == TRIGGER_CATEGORY {
            if self.activity.len() == MAX_ACTIVITY {
                self.activity.pop_front();
            }
            self.activity.push_back(Activity {
                born: self.now,
                lane: stable_hash(b"synesthesia.flight.trigger"),
                flow: stable_hash(b"synesthesia.flight.trigger"),
                category: stable_hash(TRIGGER_CATEGORY.as_bytes()),
                magnitude: 1.0,
                direction: Direction::Neutral,
            });
            return;
        }
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
        self.spawn_particles(&event, lane, flow, category);
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

    fn spawn_particles(&mut self, event: &NormalizedEvent, lane: u64, flow: u64, category: u64) {
        let style = particle_style(&event.category);
        let base = (((event.magnitude + 1.0).log2() - 7.0) * 1.5)
            .round()
            .clamp(0.0, MAX_PARTICLES_PER_EVENT as f64) as usize;
        let count = match style {
            ParticleStyle::Impact => base.max(10),
            ParticleStyle::Migration => base.max(7),
            ParticleStyle::Fracture => base.max(5),
            ParticleStyle::Ember => base,
        }
        .min(MAX_PARTICLES_PER_EVENT);
        if count == 0 {
            return;
        }

        let target_hash = event
            .target
            .as_deref()
            .map_or(flow.rotate_left(19), |target| {
                stable_hash(target.as_bytes())
            });
        let origin_y = unit_hash(lane);
        let target_y = unit_hash(target_hash);
        let anchor = unit_hash(flow);
        let base_x = match event.direction {
            Direction::Outbound => 0.12 + anchor * 0.22,
            Direction::Inbound => 0.88 - anchor * 0.22,
            Direction::Neutral | Direction::Unknown => 0.18 + anchor * 0.64,
        };
        for index in 0..count {
            let seed = mix64(
                flow ^ category.rotate_left(17)
                    ^ self.now.to_bits().rotate_left(31)
                    ^ (index as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15),
            );
            let jitter_x = unit_hash(seed) - 0.5;
            let jitter_y = unit_hash(seed.rotate_left(23)) - 0.5;
            let lifetime = match style {
                ParticleStyle::Impact => 0.25 + unit_hash(seed.rotate_left(7)) * 0.55,
                ParticleStyle::Migration => 0.7 + unit_hash(seed.rotate_left(7)) * 1.1,
                ParticleStyle::Fracture => 0.45 + unit_hash(seed.rotate_left(7)) * 1.0,
                ParticleStyle::Ember => 0.55 + unit_hash(seed.rotate_left(7)) * 1.45,
            }
            .clamp(0.25, 2.0);
            let speed = 0.08 + unit_hash(seed.rotate_left(41)) * 0.22;
            let (velocity_x, velocity_y) = match style {
                ParticleStyle::Impact => {
                    let angle = unit_hash(seed.rotate_left(11)) * std::f64::consts::TAU;
                    (angle.cos() * speed, angle.sin() * speed * 0.55)
                }
                ParticleStyle::Migration => {
                    let horizontal = match event.direction {
                        Direction::Inbound => -speed,
                        _ => speed,
                    };
                    (
                        horizontal,
                        shortest_unit_delta(origin_y, target_y) / lifetime + jitter_y * 0.08,
                    )
                }
                ParticleStyle::Fracture => {
                    let horizontal = match event.direction {
                        Direction::Inbound => -speed,
                        _ => speed,
                    };
                    (horizontal, jitter_y * 0.3)
                }
                ParticleStyle::Ember => match event.direction {
                    Direction::Inbound => (-speed, jitter_y * 0.18),
                    Direction::Outbound => (speed, jitter_y * 0.18),
                    Direction::Neutral | Direction::Unknown => {
                        (jitter_x * speed, -0.03 - unit_hash(seed) * 0.12)
                    }
                },
            };
            if self.particles.len() == MAX_PARTICLES {
                self.particles.pop_front();
                self.particle_evictions = self.particle_evictions.saturating_add(1);
            }
            self.particles.push_back(Particle {
                born: self.now,
                lifetime,
                origin_x: (base_x + jitter_x * 0.025).clamp(0.0, 1.0),
                origin_y: (origin_y + jitter_y * 0.025).rem_euclid(1.0),
                velocity_x,
                velocity_y,
                energy: (0.35 + (event.magnitude + 1.0).log2() as f32 / 18.0).clamp(0.35, 1.0),
                category,
                direction: event.direction,
                style,
                seed,
            });
        }
    }

    fn ingest_flight_metadata(&mut self, event: &NormalizedEvent) {
        let flight = self.flight.get_or_insert_with(FlightReplayMetrics::default);
        if let Some(source) = event.labels.get("source") {
            flight.source = Some(source.chars().take(16).collect());
        }
        flight.phase = match event.labels.get("record").map(String::as_str) {
            Some("start") => "pre".to_owned(),
            Some("end") => "post".to_owned(),
            _ => flight.phase.clone(),
        };
        if let Some(kind) = event.labels.get("trigger_kind") {
            flight.trigger_kind = Some(kind.chars().take(96).collect());
        }
        if let Some(reason) = event.labels.get("trigger_reason") {
            flight.trigger_reason = Some(reason.chars().take(512).collect());
        }
        if event
            .labels
            .get("record")
            .is_some_and(|value| value == "end")
        {
            flight.losses = IncidentLosses {
                kernel_ring: loss_label(event, "final_kernel_ring_loss"),
                collector: loss_label(event, "final_collector_loss"),
                ipc: loss_label(event, "final_ipc_loss"),
                renderer_channel: loss_label(event, "final_renderer_channel_loss"),
                malformed: loss_label(event, "final_malformed_count"),
                writer: loss_label(event, "final_writer_loss"),
            };
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
        while self
            .particles
            .front()
            .is_some_and(|particle| particle.born + particle.lifetime < self.now)
        {
            self.particles.pop_front();
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
        let tcp = self.tcp_metrics(elapsed);
        ModelSnapshot {
            now: self.now,
            decay_seconds: self.decay_seconds,
            activity: self.activity.iter().cloned().collect(),
            particles: self.particles.iter().cloned().collect(),
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
                tcp,
                flight: self.flight.clone(),
                particle_evictions: self.particle_evictions,
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

    fn tcp_metrics(&self, elapsed: f64) -> Option<TcpMetrics> {
        let retransmit = stable_hash(b"tcp.retransmit");
        let reset_sent = stable_hash(b"tcp.reset.send");
        let reset_received = stable_hash(b"tcp.reset.receive");
        let categories = [retransmit, reset_sent, reset_received];
        if !self
            .rates
            .iter()
            .any(|sample| categories.contains(&sample.category))
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
        let active_pathological_flows = self
            .activity
            .iter()
            .filter(|activity| categories.contains(&activity.category))
            .map(|activity| activity.flow)
            .collect::<HashSet<_>>()
            .len();
        Some(TcpMetrics {
            retransmits_per_second: rate(retransmit),
            resets_sent_per_second: rate(reset_sent),
            resets_received_per_second: rate(reset_received),
            active_pathological_flows,
        })
    }
}

fn particle_style(category: &str) -> ParticleStyle {
    if category.contains(".reset") || category.ends_with(".exit") {
        ParticleStyle::Impact
    } else if category.ends_with(".migrate") {
        ParticleStyle::Migration
    } else if category.contains("retransmit") {
        ParticleStyle::Fracture
    } else {
        ParticleStyle::Ember
    }
}

fn unit_hash(value: u64) -> f64 {
    (mix64(value) >> 11) as f64 / ((1_u64 << 53) - 1) as f64
}

fn mix64(mut value: u64) -> u64 {
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

fn shortest_unit_delta(from: f64, to: f64) -> f64 {
    let delta = to - from;
    if delta > 0.5 {
        delta - 1.0
    } else if delta < -0.5 {
        delta + 1.0
    } else {
        delta
    }
}

fn loss_label(event: &NormalizedEvent, name: &str) -> u64 {
    event
        .labels
        .get(name)
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
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

    #[test]
    fn tcp_metrics_use_aggregated_counts_and_distinct_pathology_categories() {
        let lines: Vec<_> = include_str!("../tests/fixtures/tcp-pathology.ndjson")
            .lines()
            .collect();
        let mut model = TemporalModel::default();
        for (index, count) in [(1, 3.0), (5, 1.0), (6, 1.0)] {
            let event: NormalizedEvent = serde_json::from_str(lines[index]).unwrap();
            model.ingest(event, 1.0);
            assert_eq!(model.rates.back().expect("rate sample").count, count);
        }
        let tcp = model.snapshot().metrics.tcp.unwrap();
        assert_eq!(tcp.retransmits_per_second, 30.0);
        assert_eq!(tcp.resets_sent_per_second, 10.0);
        assert_eq!(tcp.resets_received_per_second, 10.0);
        assert_eq!(tcp.active_pathological_flows, 3);
    }

    #[test]
    fn stale_tcp_pathology_decays_to_quiet() {
        let event: NormalizedEvent = serde_json::from_str(
            include_str!("../tests/fixtures/tcp-pathology.ndjson")
                .lines()
                .next()
                .unwrap(),
        )
        .unwrap();
        let mut model = TemporalModel::new(0.5);
        model.ingest(event, 0.0);
        assert!(model.snapshot().metrics.tcp.is_some());
        model.advance(2.0);
        let snapshot = model.snapshot();
        assert!(snapshot.metrics.tcp.is_none());
        assert!(snapshot.activity.is_empty());
    }

    #[test]
    fn flight_metadata_updates_phase_and_keeps_loss_boundaries_distinct() {
        let event: NormalizedEvent = serde_json::from_str(
            r#"{"v":1,"category":"synesthesia.flight.metadata","magnitude":0,"direction":"neutral","labels":{"record":"end","source":"tcp","trigger_kind":"tcp-reset","trigger_reason":"observed semantic tcp.reset.receive","final_kernel_ring_loss":"1","final_collector_loss":"2","final_ipc_loss":"3","final_renderer_channel_loss":"4","final_malformed_count":"5","final_writer_loss":"6"}}"#,
        )
        .unwrap();
        let mut model = TemporalModel::default();
        model.ingest(event, 1.0);
        let snapshot = model.snapshot();
        assert!(snapshot.activity.is_empty());
        let flight = snapshot.metrics.flight.unwrap();
        assert_eq!(flight.phase, "post");
        assert_eq!(flight.source.as_deref(), Some("tcp"));
        assert_eq!(
            flight.losses,
            IncidentLosses {
                kernel_ring: 1,
                collector: 2,
                ipc: 3,
                renderer_channel: 4,
                malformed: 5,
                writer: 6,
            }
        );
    }

    #[test]
    fn trigger_marker_is_visible_without_inventing_source_rate_or_flow() {
        let event: NormalizedEvent = serde_json::from_str(
            r#"{"v":1,"category":"synesthesia.flight.trigger","magnitude":1,"direction":"neutral","labels":{"synesthesia.flight.phase":"trigger","source":"tcp","trigger_kind":"manual","trigger_reason":"manual trigger requested"}}"#,
        )
        .unwrap();
        let mut model = TemporalModel::default();
        model.ingest(event, 1.0);
        let snapshot = model.snapshot();
        assert_eq!(snapshot.activity.len(), 1);
        assert_eq!(snapshot.metrics.events_per_second, 0.0);
        assert_eq!(snapshot.metrics.active_flows, 0);
        assert!(snapshot.metrics.tcp.is_none());
    }

    #[test]
    fn particle_spawning_is_deterministic_and_scales_with_magnitude() {
        let mut event = DemoSource::new(42).next().unwrap();
        event.magnitude = 64.0;
        let mut quiet = TemporalModel::default();
        quiet.ingest(event.clone(), 1.0);
        assert!(quiet.snapshot().particles.is_empty());

        event.magnitude = 8_192.0;
        let evolve = || {
            let mut model = TemporalModel::default();
            model.ingest(event.clone(), 1.0);
            model.snapshot().particles
        };
        let first = evolve();
        assert!(first.len() >= 8);
        assert_eq!(first, evolve());
    }

    #[test]
    fn particle_direction_controls_drift() {
        let mut event = DemoSource::new(7).next().unwrap();
        event.magnitude = 4_096.0;
        event.direction = Direction::Outbound;
        let mut outbound = TemporalModel::default();
        outbound.ingest(event.clone(), 0.0);
        assert!(
            outbound
                .snapshot()
                .particles
                .iter()
                .all(|particle| particle.velocity_x > 0.0)
        );

        event.direction = Direction::Inbound;
        let mut inbound = TemporalModel::default();
        inbound.ingest(event, 0.0);
        assert!(
            inbound
                .snapshot()
                .particles
                .iter()
                .all(|particle| particle.velocity_x < 0.0)
        );
    }

    #[test]
    fn particles_expire_and_active_storage_is_hard_bounded() {
        let mut event = DemoSource::new(9).next().unwrap();
        event.magnitude = 65_536.0;
        let mut model = TemporalModel::default();
        for _ in 0..300 {
            model.ingest(event.clone(), 0.0);
        }
        let saturated = model.snapshot();
        assert_eq!(saturated.particles.len(), MAX_PARTICLES);
        assert!(saturated.metrics.particle_evictions > 0);

        model.advance(2.1);
        assert!(model.snapshot().particles.is_empty());
    }

    #[test]
    fn source_hints_select_distinct_particle_motion_without_changing_activity() {
        let mut event = DemoSource::new(11).next().unwrap();
        event.magnitude = 8_192.0;
        event.category = "sched.migrate".to_owned();
        event.origin = Some("cpu:0".to_owned());
        event.target = Some("cpu:3".to_owned());
        event.direction = Direction::Outbound;
        let mut migration = TemporalModel::default();
        migration.ingest(event.clone(), 0.0);
        let migration_snapshot = migration.snapshot();
        assert!(
            migration_snapshot
                .particles
                .iter()
                .all(|particle| particle.style == ParticleStyle::Migration)
        );
        assert_eq!(migration_snapshot.activity.len(), 1);

        event.category = "tcp.reset.send".to_owned();
        let mut reset = TemporalModel::default();
        reset.ingest(event, 0.0);
        assert!(
            reset
                .snapshot()
                .particles
                .iter()
                .all(|particle| particle.style == ParticleStyle::Impact)
        );
    }
}
