use std::collections::BTreeMap;

use thiserror::Error;

use crate::{
    event::{Direction, NormalizedEvent, SCHEMA_VERSION},
    source::stable_hash,
};

pub const KERNEL_EVENT_BYTES: usize = 48;
pub const MAX_CPUS: usize = 4_096;
pub const UNKNOWN_CPU: u32 = u32::MAX;
pub const LANE_LABEL: &str = "synesthesia.lane";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum SchedulerEventKind {
    Switch = 1,
    Wakeup = 2,
    WakeupNew = 3,
    Migrate = 4,
}

impl TryFrom<u32> for SchedulerEventKind {
    type Error = SchedulerDecodeError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Switch),
            2 => Ok(Self::Wakeup),
            3 => Ok(Self::WakeupNew),
            4 => Ok(Self::Migrate),
            _ => Err(SchedulerDecodeError::UnsupportedKind(value)),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KernelSchedulerEvent {
    pub timestamp_ns: u64,
    pub kind: SchedulerEventKind,
    pub cpu: u32,
    pub source_cpu: u32,
    pub target_cpu: u32,
    pub pid: u32,
    pub previous_pid: u32,
    pub next_pid: u32,
    pub previous_state: i64,
}

impl KernelSchedulerEvent {
    pub fn decode(bytes: &[u8]) -> Result<Self, SchedulerDecodeError> {
        if bytes.len() != KERNEL_EVENT_BYTES {
            return Err(SchedulerDecodeError::WrongSize {
                expected: KERNEL_EVENT_BYTES,
                actual: bytes.len(),
            });
        }
        let event = Self {
            timestamp_ns: read_u64(bytes, 0),
            kind: SchedulerEventKind::try_from(read_u32(bytes, 8))?,
            cpu: read_u32(bytes, 12),
            source_cpu: read_u32(bytes, 16),
            target_cpu: read_u32(bytes, 20),
            pid: read_u32(bytes, 24),
            previous_pid: read_u32(bytes, 28),
            next_pid: read_u32(bytes, 32),
            previous_state: read_i64(bytes, 40),
        };
        for cpu in [event.cpu, event.source_cpu, event.target_cpu] {
            if cpu != UNKNOWN_CPU && cpu as usize >= MAX_CPUS {
                return Err(SchedulerDecodeError::InvalidCpu(cpu));
            }
        }
        Ok(event)
    }
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum SchedulerDecodeError {
    #[error("scheduler event has {actual} bytes; expected exactly {expected}")]
    WrongSize { expected: usize, actual: usize },
    #[error("unsupported scheduler event kind {0}")]
    UnsupportedKind(u32),
    #[error("scheduler event CPU {0} exceeds the supported bound")]
    InvalidCpu(u32),
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum SchedulerSourceError {
    #[error("the eBPF scheduler source is supported only on Linux")]
    UnsupportedOperatingSystem,
    #[error("the eBPF scheduler source is not compiled in; rebuild with --features ebpf")]
    FeatureDisabled,
    #[error("unsupported architecture {0}; this build currently supports x86_64")]
    UnsupportedArchitecture(String),
    #[error("kernel BTF is unavailable at /sys/kernel/btf/vmlinux")]
    MissingBtf,
    #[error("required scheduler tracepoint is unavailable: {0}")]
    TracepointUnavailable(String),
    #[error("insufficient eBPF permissions: {0}")]
    InsufficientPermissions(String),
    #[error("the kernel eBPF verifier rejected the scheduler program: {0}")]
    VerifierRejected(String),
    #[error("could not create or read the eBPF ring buffer: {0}")]
    RingBufferSetup(String),
    #[error("missing external eBPF build dependency: {0}")]
    MissingDevelopmentDependency(String),
    #[error("could not load the eBPF scheduler source: {0}")]
    Load(String),
}

impl SchedulerSourceError {
    pub fn classify_load_message(message: &str) -> Self {
        let lower = message.to_ascii_lowercase();
        if lower.contains("permission denied")
            || lower.contains("operation not permitted")
            || lower.contains("eperm")
        {
            Self::InsufficientPermissions(
                "run the built binary explicitly with sufficient BPF privileges".to_owned(),
            )
        } else if lower.contains("verifier") || lower.contains("invalid argument") {
            Self::VerifierRejected(message.to_owned())
        } else if lower.contains("ringbuf") || lower.contains("ring buffer") {
            Self::RingBufferSetup(message.to_owned())
        } else {
            Self::Load(message.to_owned())
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SchedulerLossStats {
    pub kernel_ring_drops: u64,
    pub userspace_channel_drops: u64,
    pub malformed_records: u64,
}

pub struct SchedulerNormalizer {
    last_switch_ns: Box<[u64; MAX_CPUS]>,
}

impl SchedulerNormalizer {
    pub fn new() -> Self {
        Self {
            last_switch_ns: Box::new([0; MAX_CPUS]),
        }
    }

    pub fn normalize(&mut self, event: KernelSchedulerEvent) -> Vec<NormalizedEvent> {
        match event.kind {
            SchedulerEventKind::Switch => vec![self.normalize_switch(event)],
            SchedulerEventKind::Wakeup | SchedulerEventKind::WakeupNew => {
                vec![self.normalize_wakeup(event)]
            }
            SchedulerEventKind::Migrate => self.normalize_migration(event),
        }
    }

    pub fn tracked_cpu_slots(&self) -> usize {
        self.last_switch_ns.len()
    }

    fn normalize_switch(&mut self, event: KernelSchedulerEvent) -> NormalizedEvent {
        let previous = self.last_switch_ns[event.cpu as usize];
        self.last_switch_ns[event.cpu as usize] = event.timestamp_ns;
        let elapsed_weight = if previous == 0 {
            0.0
        } else {
            (event.timestamp_ns.saturating_sub(previous) as f64 / 50_000.0).min(2_048.0)
        };
        let mut labels = base_labels(event.cpu);
        labels.insert("previous_pid".to_owned(), event.previous_pid.to_string());
        labels.insert("next_pid".to_owned(), event.next_pid.to_string());
        labels.insert(
            "previous_state".to_owned(),
            event.previous_state.to_string(),
        );
        labels.insert("scheduler_kind".to_owned(), "switch".to_owned());
        normalized(
            event.timestamp_ns,
            "sched.switch",
            Some(task_identity(event.previous_pid)),
            Some(task_identity(event.next_pid)),
            64.0 + elapsed_weight,
            Direction::Neutral,
            labels,
        )
    }

    fn normalize_wakeup(&self, event: KernelSchedulerEvent) -> NormalizedEvent {
        let target_cpu = if event.target_cpu == UNKNOWN_CPU {
            event.cpu
        } else {
            event.target_cpu
        };
        let mut labels = base_labels(target_cpu);
        labels.insert("pid".to_owned(), event.pid.to_string());
        labels.insert("target_cpu".to_owned(), target_cpu.to_string());
        labels.insert("scheduler_kind".to_owned(), "wakeup".to_owned());
        if event.kind == SchedulerEventKind::WakeupNew {
            labels.insert("new_task".to_owned(), "true".to_owned());
        }
        normalized(
            event.timestamp_ns,
            "sched.wakeup",
            Some(task_identity(event.previous_pid)),
            Some(task_identity(event.pid)),
            192.0,
            Direction::Inbound,
            labels,
        )
    }

    fn normalize_migration(&self, event: KernelSchedulerEvent) -> Vec<NormalizedEvent> {
        let common = [
            ("pid".to_owned(), event.pid.to_string()),
            ("source_cpu".to_owned(), event.source_cpu.to_string()),
            ("target_cpu".to_owned(), event.target_cpu.to_string()),
            ("scheduler_kind".to_owned(), "migrate".to_owned()),
        ];
        let mut departure = base_labels(event.source_cpu);
        departure.extend(common.clone());
        departure.insert("migration_phase".to_owned(), "depart".to_owned());
        let mut arrival = base_labels(event.target_cpu);
        arrival.extend(common);
        arrival.insert("migration_phase".to_owned(), "arrive".to_owned());
        vec![
            normalized(
                event.timestamp_ns,
                "sched.migrate",
                Some(cpu_identity(event.source_cpu)),
                Some(cpu_identity(event.target_cpu)),
                2_048.0,
                Direction::Outbound,
                departure,
            ),
            normalized(
                event.timestamp_ns,
                "sched.migrate",
                Some(cpu_identity(event.target_cpu)),
                Some(cpu_identity(event.source_cpu)),
                1_536.0,
                Direction::Inbound,
                arrival,
            ),
        ]
    }
}

impl Default for SchedulerNormalizer {
    fn default() -> Self {
        Self::new()
    }
}

fn normalized(
    timestamp_ns: u64,
    category: &str,
    origin: Option<String>,
    target: Option<String>,
    magnitude: f64,
    direction: Direction,
    labels: BTreeMap<String, String>,
) -> NormalizedEvent {
    NormalizedEvent {
        v: SCHEMA_VERSION,
        timestamp: Some(timestamp_ns as f64 / 1_000_000_000.0),
        category: category.to_owned(),
        origin,
        target,
        magnitude,
        direction,
        labels,
    }
}

fn base_labels(cpu: u32) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("cpu".to_owned(), cpu.to_string()),
        (LANE_LABEL.to_owned(), cpu_identity(cpu)),
    ])
}

fn cpu_identity(cpu: u32) -> String {
    format!("cpu:{cpu}")
}

fn task_identity(pid: u32) -> String {
    if pid == 0 {
        "idle".to_owned()
    } else {
        format!("task:{pid}")
    }
}

pub fn cpu_lane_hash(cpu: u32) -> u64 {
    stable_hash(cpu_identity(cpu).as_bytes())
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_ne_bytes(bytes[offset..offset + 4].try_into().expect("checked size"))
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_ne_bytes(bytes[offset..offset + 8].try_into().expect("checked size"))
}

fn read_i64(bytes: &[u8], offset: usize) -> i64 {
    i64::from_ne_bytes(bytes[offset..offset + 8].try_into().expect("checked size"))
}

#[cfg(test)]
mod tests {
    use crate::model::TemporalModel;

    use super::*;

    fn encoded(event: KernelSchedulerEvent) -> [u8; KERNEL_EVENT_BYTES] {
        let mut bytes = [0; KERNEL_EVENT_BYTES];
        bytes[0..8].copy_from_slice(&event.timestamp_ns.to_ne_bytes());
        bytes[8..12].copy_from_slice(&(event.kind as u32).to_ne_bytes());
        bytes[12..16].copy_from_slice(&event.cpu.to_ne_bytes());
        bytes[16..20].copy_from_slice(&event.source_cpu.to_ne_bytes());
        bytes[20..24].copy_from_slice(&event.target_cpu.to_ne_bytes());
        bytes[24..28].copy_from_slice(&event.pid.to_ne_bytes());
        bytes[28..32].copy_from_slice(&event.previous_pid.to_ne_bytes());
        bytes[32..36].copy_from_slice(&event.next_pid.to_ne_bytes());
        bytes[40..48].copy_from_slice(&event.previous_state.to_ne_bytes());
        bytes
    }

    fn switch(cpu: u32, previous_pid: u32, next_pid: u32) -> KernelSchedulerEvent {
        KernelSchedulerEvent {
            timestamp_ns: 1_000_000,
            kind: SchedulerEventKind::Switch,
            cpu,
            source_cpu: cpu,
            target_cpu: cpu,
            pid: next_pid,
            previous_pid,
            next_pid,
            previous_state: 1,
        }
    }

    #[test]
    fn decodes_exact_kernel_event_structure() {
        let expected = KernelSchedulerEvent {
            timestamp_ns: 9_876_543_210,
            kind: SchedulerEventKind::Migrate,
            cpu: 2,
            source_cpu: 2,
            target_cpu: 7,
            pid: 4242,
            previous_pid: 0,
            next_pid: 0,
            previous_state: -1,
        };
        assert_eq!(
            KernelSchedulerEvent::decode(&encoded(expected)),
            Ok(expected)
        );
    }

    #[test]
    fn sanitized_kernel_fixture_covers_all_event_kinds() {
        let mut seen = Vec::new();
        for line in include_str!("../../tests/fixtures/scheduler-kernel-events.tsv").lines() {
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            let fields: Vec<_> = line.split('\t').collect();
            assert_eq!(fields.len(), 9);
            let raw_kind = fields[1].parse::<u32>().unwrap();
            let event = KernelSchedulerEvent {
                timestamp_ns: fields[0].parse().unwrap(),
                kind: SchedulerEventKind::try_from(raw_kind).unwrap(),
                cpu: fields[2].parse().unwrap(),
                source_cpu: fields[3].parse().unwrap(),
                target_cpu: fields[4].parse().unwrap(),
                pid: fields[5].parse().unwrap(),
                previous_pid: fields[6].parse().unwrap(),
                next_pid: fields[7].parse().unwrap(),
                previous_state: fields[8].parse().unwrap(),
            };
            seen.push(KernelSchedulerEvent::decode(&encoded(event)).unwrap().kind);
        }
        assert_eq!(
            seen,
            [
                SchedulerEventKind::Switch,
                SchedulerEventKind::Wakeup,
                SchedulerEventKind::WakeupNew,
                SchedulerEventKind::Migrate,
            ]
        );
    }

    #[test]
    fn refuses_malformed_oversized_and_unknown_events() {
        assert!(matches!(
            KernelSchedulerEvent::decode(&[0; KERNEL_EVENT_BYTES - 1]),
            Err(SchedulerDecodeError::WrongSize { .. })
        ));
        assert!(matches!(
            KernelSchedulerEvent::decode(&[0; KERNEL_EVENT_BYTES + 1]),
            Err(SchedulerDecodeError::WrongSize { .. })
        ));
        let mut unknown = encoded(switch(1, 2, 3));
        unknown[8..12].copy_from_slice(&99_u32.to_ne_bytes());
        assert_eq!(
            KernelSchedulerEvent::decode(&unknown),
            Err(SchedulerDecodeError::UnsupportedKind(99))
        );
    }

    #[test]
    fn switch_and_wakeup_map_to_distinct_categories() {
        let mut normalizer = SchedulerNormalizer::new();
        let switched = normalizer.normalize(switch(3, 10, 11));
        let wakeup = normalizer.normalize(KernelSchedulerEvent {
            timestamp_ns: 2_000_000,
            kind: SchedulerEventKind::Wakeup,
            cpu: 1,
            source_cpu: 1,
            target_cpu: 3,
            pid: 12,
            previous_pid: 10,
            next_pid: 0,
            previous_state: 0,
        });
        assert_eq!(switched[0].category, "sched.switch");
        assert_eq!(switched[0].direction, Direction::Neutral);
        assert_eq!(wakeup[0].category, "sched.wakeup");
        assert_eq!(wakeup[0].direction, Direction::Inbound);
        assert_eq!(wakeup[0].labels["target_cpu"], "3");
    }

    #[test]
    fn cpu_lane_is_stable_across_transient_tasks() {
        let mut normalizer = SchedulerNormalizer::new();
        let first = normalizer.normalize(switch(4, 100, 101)).remove(0);
        let second = normalizer.normalize(switch(4, 8_000, 9_000)).remove(0);
        let other = normalizer.normalize(switch(5, 100, 101)).remove(0);
        let mut model = TemporalModel::default();
        model.ingest(first, 0.0);
        model.ingest(second, 0.1);
        model.ingest(other, 0.2);
        let snapshot = model.snapshot();
        assert_eq!(snapshot.activity[0].lane, snapshot.activity[1].lane);
        assert_ne!(snapshot.activity[1].lane, snapshot.activity[2].lane);
        assert_eq!(snapshot.activity[0].lane, cpu_lane_hash(4));
    }

    #[test]
    fn migration_emits_departure_and_arrival_lanes() {
        let events = SchedulerNormalizer::new().normalize(KernelSchedulerEvent {
            timestamp_ns: 3_000_000,
            kind: SchedulerEventKind::Migrate,
            cpu: 1,
            source_cpu: 1,
            target_cpu: 6,
            pid: 77,
            previous_pid: 0,
            next_pid: 0,
            previous_state: 0,
        });
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].labels["migration_phase"], "depart");
        assert_eq!(events[0].labels[LANE_LABEL], "cpu:1");
        assert_eq!(events[1].labels["migration_phase"], "arrive");
        assert_eq!(events[1].labels[LANE_LABEL], "cpu:6");
        assert_ne!(events[0].direction, events[1].direction);
    }

    #[test]
    fn normalizer_state_is_cpu_bounded_not_task_bounded() {
        let mut normalizer = SchedulerNormalizer::new();
        for pid in 1..20_000 {
            let events = normalizer.normalize(switch(pid % 8, pid, pid + 1));
            assert!(events[0].origin.as_ref().unwrap().len() < 32);
            assert!(events[0].target.as_ref().unwrap().len() < 32);
        }
        assert_eq!(normalizer.tracked_cpu_slots(), MAX_CPUS);
    }

    #[test]
    fn load_errors_are_classified_without_privilege() {
        assert!(matches!(
            SchedulerSourceError::classify_load_message("Operation not permitted (os error 1)"),
            SchedulerSourceError::InsufficientPermissions(_)
        ));
        assert!(matches!(
            SchedulerSourceError::classify_load_message("verifier rejected instruction 12"),
            SchedulerSourceError::VerifierRejected(_)
        ));
        assert!(matches!(
            SchedulerSourceError::classify_load_message("ring buffer map missing"),
            SchedulerSourceError::RingBufferSetup(_)
        ));
    }

    #[test]
    fn kernel_and_userspace_loss_counters_remain_separate() {
        let stats = SchedulerLossStats {
            kernel_ring_drops: 11,
            userspace_channel_drops: 7,
            malformed_records: 3,
        };
        assert_eq!(stats.kernel_ring_drops, 11);
        assert_eq!(stats.userspace_channel_drops, 7);
        assert_eq!(stats.malformed_records, 3);
    }
}
