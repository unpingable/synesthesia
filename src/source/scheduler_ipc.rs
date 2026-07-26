use std::{
    collections::BTreeMap,
    io::{self, Read, Write},
};

use thiserror::Error;

use crate::{
    event::{Direction, NormalizedEvent, SCHEMA_VERSION},
    source::scheduler::{KernelSchedulerEvent, SchedulerEventKind, UNKNOWN_CPU},
};

pub const WIRE_BYTES: usize = 64;
pub const WIRE_MAGIC: [u8; 4] = *b"SYNB";
pub const WIRE_VERSION: u16 = 1;
pub const MAX_PULSES_PER_WINDOW: usize = 2_048;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum PulseKind {
    Heartbeat = 0,
    Switch = 1,
    Wakeup = 2,
    MigrateDepart = 3,
    MigrateArrive = 4,
}

impl TryFrom<u8> for PulseKind {
    type Error = SchedulerWireError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Heartbeat),
            1 => Ok(Self::Switch),
            2 => Ok(Self::Wakeup),
            3 => Ok(Self::MigrateDepart),
            4 => Ok(Self::MigrateArrive),
            _ => Err(SchedulerWireError::UnsupportedKind(value)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NormalizedSchedulerPulse {
    pub timestamp_ns: u64,
    pub kind: PulseKind,
    pub lane_cpu: u32,
    pub source_cpu: u32,
    pub target_cpu: u32,
    pub origin_pid: u32,
    pub target_pid: u32,
    pub event_count: u32,
    pub magnitude: f64,
    pub kernel_ring_drops: u64,
    pub collector_drops: u64,
}

impl NormalizedSchedulerPulse {
    pub fn write_to(&self, writer: &mut impl Write) -> io::Result<()> {
        writer.write_all(&self.encode())
    }

    pub fn read_from(reader: &mut impl Read) -> Result<Option<Self>, SchedulerWireError> {
        let mut bytes = [0; WIRE_BYTES];
        let mut read = 0;
        while read < bytes.len() {
            match reader.read(&mut bytes[read..]) {
                Ok(0) if read == 0 => return Ok(None),
                Ok(0) => return Err(SchedulerWireError::Truncated(read)),
                Ok(count) => read += count,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(SchedulerWireError::Io(error)),
            }
        }
        Self::decode(&bytes).map(Some)
    }

    pub fn encode(&self) -> [u8; WIRE_BYTES] {
        let mut bytes = [0; WIRE_BYTES];
        bytes[0..4].copy_from_slice(&WIRE_MAGIC);
        bytes[4..6].copy_from_slice(&WIRE_VERSION.to_le_bytes());
        bytes[6] = self.kind as u8;
        bytes[8..16].copy_from_slice(&self.timestamp_ns.to_le_bytes());
        bytes[16..20].copy_from_slice(&self.lane_cpu.to_le_bytes());
        bytes[20..24].copy_from_slice(&self.source_cpu.to_le_bytes());
        bytes[24..28].copy_from_slice(&self.target_cpu.to_le_bytes());
        bytes[28..32].copy_from_slice(&self.origin_pid.to_le_bytes());
        bytes[32..36].copy_from_slice(&self.target_pid.to_le_bytes());
        bytes[36..40].copy_from_slice(&self.event_count.to_le_bytes());
        bytes[40..48].copy_from_slice(&self.magnitude.to_le_bytes());
        bytes[48..56].copy_from_slice(&self.kernel_ring_drops.to_le_bytes());
        bytes[56..64].copy_from_slice(&self.collector_drops.to_le_bytes());
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, SchedulerWireError> {
        if bytes.len() != WIRE_BYTES {
            return Err(SchedulerWireError::WrongSize(bytes.len()));
        }
        if bytes[0..4] != WIRE_MAGIC {
            return Err(SchedulerWireError::BadMagic);
        }
        let version = read_u16(bytes, 4);
        if version != WIRE_VERSION {
            return Err(SchedulerWireError::UnsupportedVersion(version));
        }
        Ok(Self {
            timestamp_ns: read_u64(bytes, 8),
            kind: PulseKind::try_from(bytes[6])?,
            lane_cpu: read_u32(bytes, 16),
            source_cpu: read_u32(bytes, 20),
            target_cpu: read_u32(bytes, 24),
            origin_pid: read_u32(bytes, 28),
            target_pid: read_u32(bytes, 32),
            event_count: read_u32(bytes, 36),
            magnitude: f64::from_le_bytes(bytes[40..48].try_into().expect("checked size")),
            kernel_ring_drops: read_u64(bytes, 48),
            collector_drops: read_u64(bytes, 56),
        })
    }

    pub fn into_normalized(self) -> Option<NormalizedEvent> {
        if self.kind == PulseKind::Heartbeat {
            return None;
        }
        let (category, direction, phase) = match self.kind {
            PulseKind::Switch => ("sched.switch", Direction::Neutral, None),
            PulseKind::Wakeup => ("sched.wakeup", Direction::Inbound, None),
            PulseKind::MigrateDepart => ("sched.migrate", Direction::Outbound, Some("depart")),
            PulseKind::MigrateArrive => ("sched.migrate", Direction::Inbound, Some("arrive")),
            PulseKind::Heartbeat => unreachable!(),
        };
        let mut labels = BTreeMap::from([
            ("cpu".to_owned(), self.lane_cpu.to_string()),
            (
                "synesthesia.lane".to_owned(),
                format!("cpu:{}", self.lane_cpu),
            ),
            ("event_count".to_owned(), self.event_count.to_string()),
            ("source_cpu".to_owned(), self.source_cpu.to_string()),
            ("target_cpu".to_owned(), self.target_cpu.to_string()),
            ("scheduler_kind".to_owned(), category.to_owned()),
        ]);
        labels.insert(
            "synesthesia.event_count".to_owned(),
            if self.kind == PulseKind::MigrateArrive {
                "0".to_owned()
            } else {
                self.event_count.to_string()
            },
        );
        if let Some(phase) = phase {
            labels.insert("migration_phase".to_owned(), phase.to_owned());
        }
        Some(NormalizedEvent {
            v: SCHEMA_VERSION,
            timestamp: Some(self.timestamp_ns as f64 / 1_000_000_000.0),
            category: category.to_owned(),
            origin: identity(self.origin_pid, self.source_cpu),
            target: identity(self.target_pid, self.target_cpu),
            magnitude: self.magnitude,
            direction,
            labels,
        })
    }
}

fn identity(pid: u32, cpu: u32) -> Option<String> {
    if pid != 0 {
        Some(format!("task:{pid}"))
    } else if cpu != UNKNOWN_CPU {
        Some(format!("cpu:{cpu}"))
    } else {
        None
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Bucket {
    count: u32,
    magnitude: f64,
    source_cpu: u32,
    target_cpu: u32,
    origin_pid: u32,
    target_pid: u32,
}

pub struct SchedulerAggregator {
    buckets: BTreeMap<(u32, u8), Bucket>,
    collector_drops: u64,
}

impl SchedulerAggregator {
    pub fn new() -> Self {
        Self {
            buckets: BTreeMap::new(),
            collector_drops: 0,
        }
    }

    pub fn ingest(&mut self, event: KernelSchedulerEvent) {
        match event.kind {
            SchedulerEventKind::Switch => self.add(
                event.cpu,
                PulseKind::Switch,
                event.cpu,
                event.cpu,
                event.previous_pid,
                event.next_pid,
                64.0,
            ),
            SchedulerEventKind::Wakeup | SchedulerEventKind::WakeupNew => {
                let target = if event.target_cpu == UNKNOWN_CPU {
                    event.cpu
                } else {
                    event.target_cpu
                };
                self.add(
                    target,
                    PulseKind::Wakeup,
                    event.cpu,
                    target,
                    event.previous_pid,
                    event.pid,
                    192.0,
                );
            }
            SchedulerEventKind::Migrate => {
                self.add(
                    event.source_cpu,
                    PulseKind::MigrateDepart,
                    event.source_cpu,
                    event.target_cpu,
                    event.pid,
                    0,
                    2_048.0,
                );
                self.add(
                    event.target_cpu,
                    PulseKind::MigrateArrive,
                    event.source_cpu,
                    event.target_cpu,
                    0,
                    event.pid,
                    1_536.0,
                );
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn add(
        &mut self,
        lane_cpu: u32,
        kind: PulseKind,
        source_cpu: u32,
        target_cpu: u32,
        origin_pid: u32,
        target_pid: u32,
        magnitude: f64,
    ) {
        let key = (lane_cpu, kind as u8);
        if !self.buckets.contains_key(&key) && self.buckets.len() >= MAX_PULSES_PER_WINDOW {
            self.collector_drops = self.collector_drops.saturating_add(1);
            return;
        }
        let bucket = self.buckets.entry(key).or_default();
        bucket.count = bucket.count.saturating_add(1);
        bucket.magnitude = (bucket.magnitude + magnitude).min(1_000_000.0);
        bucket.source_cpu = source_cpu;
        bucket.target_cpu = target_cpu;
        bucket.origin_pid = origin_pid;
        bucket.target_pid = target_pid;
    }

    pub fn flush(
        &mut self,
        timestamp_ns: u64,
        kernel_ring_drops: u64,
    ) -> Vec<NormalizedSchedulerPulse> {
        if self.buckets.is_empty() {
            return vec![NormalizedSchedulerPulse {
                timestamp_ns,
                kind: PulseKind::Heartbeat,
                lane_cpu: 0,
                source_cpu: UNKNOWN_CPU,
                target_cpu: UNKNOWN_CPU,
                origin_pid: 0,
                target_pid: 0,
                event_count: 0,
                magnitude: 0.0,
                kernel_ring_drops,
                collector_drops: self.collector_drops,
            }];
        }
        let collector_drops = self.collector_drops;
        std::mem::take(&mut self.buckets)
            .into_iter()
            .map(|((lane_cpu, kind), bucket)| NormalizedSchedulerPulse {
                timestamp_ns,
                kind: PulseKind::try_from(kind).expect("internally generated kind"),
                lane_cpu,
                source_cpu: bucket.source_cpu,
                target_cpu: bucket.target_cpu,
                origin_pid: bucket.origin_pid,
                target_pid: bucket.target_pid,
                event_count: bucket.count,
                magnitude: bucket.magnitude,
                kernel_ring_drops,
                collector_drops,
            })
            .collect()
    }

    pub fn bucket_count(&self) -> usize {
        self.buckets.len()
    }
}

impl Default for SchedulerAggregator {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Error)]
pub enum SchedulerWireError {
    #[error("scheduler helper record has {0} bytes; expected {WIRE_BYTES}")]
    WrongSize(usize),
    #[error("scheduler helper record has invalid magic")]
    BadMagic,
    #[error("unsupported scheduler helper protocol version {0}")]
    UnsupportedVersion(u16),
    #[error("unsupported scheduler pulse kind {0}")]
    UnsupportedKind(u8),
    #[error("scheduler helper stream ended after {0} bytes of a record")]
    Truncated(usize),
    #[error("scheduler helper I/O failed: {0}")]
    Io(#[from] io::Error),
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().expect("checked size"))
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("checked size"))
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().expect("checked size"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kernel(kind: SchedulerEventKind, cpu: u32) -> KernelSchedulerEvent {
        KernelSchedulerEvent {
            timestamp_ns: 1_000_000,
            kind,
            cpu,
            source_cpu: cpu,
            target_cpu: cpu + 1,
            pid: 30,
            previous_pid: 10,
            next_pid: 20,
            previous_state: 0,
        }
    }

    #[test]
    fn binary_protocol_round_trips_without_json() {
        let expected = NormalizedSchedulerPulse {
            timestamp_ns: 123,
            kind: PulseKind::MigrateArrive,
            lane_cpu: 3,
            source_cpu: 2,
            target_cpu: 3,
            origin_pid: 7,
            target_pid: 8,
            event_count: 9,
            magnitude: 42.5,
            kernel_ring_drops: 11,
            collector_drops: 12,
        };
        assert_eq!(
            NormalizedSchedulerPulse::decode(&expected.encode()).unwrap(),
            expected
        );
    }

    #[test]
    fn aggregation_occurs_before_normalized_event_materialization() {
        let mut aggregator = SchedulerAggregator::new();
        for _ in 0..10 {
            aggregator.ingest(kernel(SchedulerEventKind::Switch, 2));
        }
        let pulses = aggregator.flush(2_000_000, 0);
        assert_eq!(pulses.len(), 1);
        assert_eq!(pulses[0].event_count, 10);
        let event = pulses[0].into_normalized().unwrap();
        assert_eq!(event.category, "sched.switch");
        assert_eq!(event.labels["synesthesia.lane"], "cpu:2");
    }

    #[test]
    fn migrations_aggregate_into_both_cpu_lanes() {
        let mut aggregator = SchedulerAggregator::new();
        aggregator.ingest(kernel(SchedulerEventKind::Migrate, 2));
        let pulses = aggregator.flush(2_000_000, 0);
        assert_eq!(pulses.len(), 2);
        assert!(pulses.iter().any(|pulse| pulse.lane_cpu == 2));
        assert!(pulses.iter().any(|pulse| pulse.lane_cpu == 3));
    }

    #[test]
    fn aggregation_cardinality_is_bounded() {
        let mut aggregator = SchedulerAggregator::new();
        for cpu in 0..10_000 {
            aggregator.ingest(kernel(SchedulerEventKind::Switch, cpu));
        }
        assert_eq!(aggregator.bucket_count(), MAX_PULSES_PER_WINDOW);
        assert!(
            aggregator
                .flush(2_000_000, 0)
                .iter()
                .all(|pulse| pulse.collector_drops > 0)
        );
    }

    #[test]
    fn malformed_wire_records_are_refused() {
        let pulse = SchedulerAggregator::new().flush(0, 0).remove(0);
        let mut bytes = pulse.encode();
        bytes[0] = b'X';
        assert!(matches!(
            NormalizedSchedulerPulse::decode(&bytes),
            Err(SchedulerWireError::BadMagic)
        ));
        bytes = pulse.encode();
        bytes[4..6].copy_from_slice(&99_u16.to_le_bytes());
        assert!(matches!(
            NormalizedSchedulerPulse::decode(&bytes),
            Err(SchedulerWireError::UnsupportedVersion(99))
        ));
    }
}
