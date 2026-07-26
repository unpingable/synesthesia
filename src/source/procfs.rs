use std::collections::BTreeMap;

use crate::{
    event::{Direction, NormalizedEvent, SCHEMA_VERSION},
    source::stable_hash,
};

pub const DEFAULT_SAMPLE_INTERVAL_MS: u64 = 250;
pub const MAX_SCANNED_PROCESSES: usize = 8_192;
pub const MAX_TRACKED_PROCESSES: usize = 4_096;
pub const MAX_EVENTS_PER_SAMPLE: usize = 8_192;
const MAX_COMM_CHARS: usize = 64;
const MAX_DELTA_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessSample {
    pub pid: u32,
    pub comm: String,
    pub cpu_ticks: u64,
    pub start_ticks: u64,
    pub read_bytes: Option<u64>,
    pub write_bytes: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProcStat {
    pub total_cpu_ticks: u64,
    pub idle_cpu_ticks: u64,
    pub procs_running: u32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MemorySample {
    pub total_kib: u64,
    pub available_kib: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProcSample {
    pub timestamp: f64,
    pub processes: Vec<ProcessSample>,
    pub stat: ProcStat,
    pub memory: Option<MemorySample>,
    pub unreadable: u64,
    pub vanished: u64,
    pub scan_refused: u64,
}

impl ProcSample {
    pub fn new(timestamp: f64) -> Self {
        Self {
            timestamp,
            ..Self::default()
        }
    }

    pub fn push_process(&mut self, process: ProcessSample) -> bool {
        if self.processes.len() == MAX_SCANNED_PROCESSES {
            self.scan_refused = self.scan_refused.saturating_add(1);
            return false;
        }
        self.processes.push(process);
        true
    }

    fn sort_processes(&mut self) {
        self.processes
            .sort_unstable_by_key(|process| (process.pid, process.start_ticks));
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProcStats {
    pub samples: u64,
    pub tracked_processes: usize,
    pub cpu_events: u64,
    pub io_events: u64,
    pub starts: u64,
    pub exits: u64,
    pub unreadable: u64,
    pub vanished: u64,
    pub scan_refused: u64,
    pub tracking_refused: u64,
    pub event_refused: u64,
    pub counter_resets: u64,
}

#[derive(Clone, Debug)]
struct TrackedProcess {
    comm: String,
    start_ticks: u64,
    cpu_ticks: u64,
    read_bytes: Option<u64>,
    write_bytes: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct ProcTracker {
    tracked: BTreeMap<u32, TrackedProcess>,
    stats: ProcStats,
    anonymize: bool,
    session_seed: u64,
    last_running: Option<u32>,
    last_memory_band: Option<u8>,
    initialized: bool,
}

impl ProcTracker {
    pub fn new(anonymize: bool, session_seed: u64) -> Self {
        Self {
            tracked: BTreeMap::new(),
            stats: ProcStats::default(),
            anonymize,
            session_seed,
            last_running: None,
            last_memory_band: None,
            initialized: false,
        }
    }

    pub fn ingest(&mut self, mut sample: ProcSample) -> Vec<NormalizedEvent> {
        sample.sort_processes();
        self.stats.samples = self.stats.samples.saturating_add(1);
        self.stats.unreadable = self.stats.unreadable.saturating_add(sample.unreadable);
        self.stats.vanished = self.stats.vanished.saturating_add(sample.vanished);
        self.stats.scan_refused = self.stats.scan_refused.saturating_add(sample.scan_refused);
        if !self.initialized {
            for process in &sample.processes {
                if self.tracked.len() == MAX_TRACKED_PROCESSES {
                    self.stats.tracking_refused = self.stats.tracking_refused.saturating_add(1);
                    continue;
                }
                self.tracked.insert(
                    process.pid,
                    TrackedProcess {
                        comm: bounded_comm(&process.comm),
                        start_ticks: process.start_ticks,
                        cpu_ticks: process.cpu_ticks,
                        read_bytes: process.read_bytes,
                        write_bytes: process.write_bytes,
                    },
                );
            }
            self.stats.tracked_processes = self.tracked.len();
            self.last_running = Some(sample.stat.procs_running);
            self.last_memory_band = sample.memory.map(memory_band);
            self.initialized = true;
            return Vec::new();
        }

        let present: BTreeMap<u32, u64> = sample
            .processes
            .iter()
            .take(MAX_SCANNED_PROCESSES)
            .map(|process| (process.pid, process.start_ticks))
            .collect();
        let exited: Vec<u32> = self
            .tracked
            .iter()
            .filter(|(pid, tracked)| {
                present
                    .get(pid)
                    .is_none_or(|start_ticks| *start_ticks != tracked.start_ticks)
            })
            .map(|(pid, _)| *pid)
            .collect();

        let mut events = Vec::with_capacity(
            sample
                .processes
                .len()
                .saturating_mul(2)
                .min(MAX_EVENTS_PER_SAMPLE),
        );
        for pid in exited {
            if let Some(process) = self.tracked.remove(&pid) {
                let identity = self.identity(pid, process.start_ticks, &process.comm);
                self.push_event(
                    &mut events,
                    process_event(
                        sample.timestamp,
                        "proc.exit",
                        Some(identity),
                        None,
                        2_048.0,
                        Direction::Outbound,
                        ProcessEventIdentity {
                            pid,
                            start_ticks: process.start_ticks,
                            comm: &process.comm,
                            anonymize: self.anonymize,
                        },
                    ),
                );
                self.stats.exits = self.stats.exits.saturating_add(1);
            }
        }

        let mut new_processes = Vec::new();
        for process in &sample.processes {
            if !self.tracked.contains_key(&process.pid) {
                new_processes.push(process);
                continue;
            }
            let identity = identity_for(
                self.anonymize,
                self.session_seed,
                process.pid,
                process.start_ticks,
                &process.comm,
            );
            let mut pending = Vec::with_capacity(3);
            {
                let previous = self
                    .tracked
                    .get_mut(&process.pid)
                    .expect("presence checked above");
                let cpu_delta = monotonic_delta(
                    previous.cpu_ticks,
                    process.cpu_ticks,
                    &mut self.stats.counter_resets,
                );
                if cpu_delta > 0 {
                    pending.push(process_event(
                        sample.timestamp,
                        "proc.cpu",
                        Some(identity.clone()),
                        Some("host:cpu".to_owned()),
                        (cpu_delta.saturating_mul(128)).min(1 << 20) as f64,
                        Direction::Neutral,
                        ProcessEventIdentity {
                            pid: process.pid,
                            start_ticks: process.start_ticks,
                            comm: &process.comm,
                            anonymize: self.anonymize,
                        },
                    ));
                    self.stats.cpu_events = self.stats.cpu_events.saturating_add(1);
                }
                if let Some(delta) = optional_delta(
                    previous.read_bytes,
                    process.read_bytes,
                    &mut self.stats.counter_resets,
                ) {
                    if delta > 0 {
                        pending.push(process_event(
                            sample.timestamp,
                            "proc.io.read",
                            Some("host:storage".to_owned()),
                            Some(identity.clone()),
                            delta.min(MAX_DELTA_BYTES) as f64,
                            Direction::Inbound,
                            ProcessEventIdentity {
                                pid: process.pid,
                                start_ticks: process.start_ticks,
                                comm: &process.comm,
                                anonymize: self.anonymize,
                            },
                        ));
                        self.stats.io_events = self.stats.io_events.saturating_add(1);
                    }
                }
                if let Some(delta) = optional_delta(
                    previous.write_bytes,
                    process.write_bytes,
                    &mut self.stats.counter_resets,
                ) {
                    if delta > 0 {
                        pending.push(process_event(
                            sample.timestamp,
                            "proc.io.write",
                            Some(identity.clone()),
                            Some("host:storage".to_owned()),
                            delta.min(MAX_DELTA_BYTES) as f64,
                            Direction::Outbound,
                            ProcessEventIdentity {
                                pid: process.pid,
                                start_ticks: process.start_ticks,
                                comm: &process.comm,
                                anonymize: self.anonymize,
                            },
                        ));
                        self.stats.io_events = self.stats.io_events.saturating_add(1);
                    }
                }
                previous.comm = bounded_comm(&process.comm);
                previous.cpu_ticks = process.cpu_ticks;
                previous.read_bytes = process.read_bytes;
                previous.write_bytes = process.write_bytes;
            }
            for event in pending {
                self.push_event(&mut events, event);
            }
        }

        let mut untracked = sample.scan_refused;
        for process in new_processes {
            if self.tracked.len() == MAX_TRACKED_PROCESSES {
                self.stats.tracking_refused = self.stats.tracking_refused.saturating_add(1);
                untracked = untracked.saturating_add(1);
                continue;
            }
            let comm = bounded_comm(&process.comm);
            let identity = self.identity(process.pid, process.start_ticks, &comm);
            self.tracked.insert(
                process.pid,
                TrackedProcess {
                    comm: comm.clone(),
                    start_ticks: process.start_ticks,
                    cpu_ticks: process.cpu_ticks,
                    read_bytes: process.read_bytes,
                    write_bytes: process.write_bytes,
                },
            );
            self.push_event(
                &mut events,
                process_event(
                    sample.timestamp,
                    "proc.start",
                    None,
                    Some(identity),
                    512.0,
                    Direction::Inbound,
                    ProcessEventIdentity {
                        pid: process.pid,
                        start_ticks: process.start_ticks,
                        comm: &comm,
                        anonymize: self.anonymize,
                    },
                ),
            );
            self.stats.starts = self.stats.starts.saturating_add(1);
        }

        if untracked > 0 {
            self.push_event(
                &mut events,
                host_event(
                    sample.timestamp,
                    "proc.background",
                    untracked.min(1 << 20) as f64,
                    "untracked_processes",
                    untracked,
                ),
            );
        }
        if self.last_running != Some(sample.stat.procs_running) {
            self.push_event(
                &mut events,
                host_event(
                    sample.timestamp,
                    "host.runqueue",
                    f64::from(sample.stat.procs_running).max(1.0) * 256.0,
                    "procs_running",
                    u64::from(sample.stat.procs_running),
                ),
            );
            self.last_running = Some(sample.stat.procs_running);
        }
        if let Some(memory) = sample.memory {
            let available = if memory.total_kib == 0 {
                1.0
            } else {
                memory.available_kib as f64 / memory.total_kib as f64
            };
            let band = (available.clamp(0.0, 1.0) * 20.0).floor() as u8;
            if available < 0.15 && self.last_memory_band != Some(band) {
                self.push_event(
                    &mut events,
                    host_event(
                        sample.timestamp,
                        "host.memory.pressure",
                        ((1.0 - available) * 4_096.0).clamp(1.0, 4_096.0),
                        "available_percent",
                        (available * 100.0).round() as u64,
                    ),
                );
            }
            self.last_memory_band = Some(band);
        }

        self.stats.tracked_processes = self.tracked.len();
        events
    }

    pub fn stats(&self) -> ProcStats {
        self.stats
    }

    fn identity(&self, pid: u32, start_ticks: u64, comm: &str) -> String {
        identity_for(self.anonymize, self.session_seed, pid, start_ticks, comm)
    }

    fn push_event(&mut self, events: &mut Vec<NormalizedEvent>, event: NormalizedEvent) {
        if events.len() == MAX_EVENTS_PER_SAMPLE {
            self.stats.event_refused = self.stats.event_refused.saturating_add(1);
        } else {
            events.push(event);
        }
    }
}

#[derive(Clone, Copy)]
struct ProcessEventIdentity<'a> {
    pid: u32,
    start_ticks: u64,
    comm: &'a str,
    anonymize: bool,
}

fn memory_band(memory: MemorySample) -> u8 {
    if memory.total_kib == 0 {
        20
    } else {
        ((memory.available_kib as f64 / memory.total_kib as f64).clamp(0.0, 1.0) * 20.0).floor()
            as u8
    }
}

fn process_event(
    timestamp: f64,
    category: &str,
    origin: Option<String>,
    target: Option<String>,
    magnitude: f64,
    direction: Direction,
    process: ProcessEventIdentity<'_>,
) -> NormalizedEvent {
    let identity = origin
        .as_deref()
        .filter(|value| value.starts_with("proc:"))
        .or_else(|| target.as_deref().filter(|value| value.starts_with("proc:")))
        .unwrap_or("proc:unknown");
    let mut labels = BTreeMap::new();
    labels.insert("synesthesia.lane".to_owned(), identity.to_owned());
    labels.insert(
        "process_start_ticks".to_owned(),
        process.start_ticks.to_string(),
    );
    if !process.anonymize {
        labels.insert("pid".to_owned(), process.pid.to_string());
        labels.insert("comm".to_owned(), bounded_comm(process.comm));
    }
    NormalizedEvent {
        v: SCHEMA_VERSION,
        timestamp: Some(timestamp),
        category: category.to_owned(),
        origin,
        target,
        magnitude,
        direction,
        labels,
    }
}

fn host_event(
    timestamp: f64,
    category: &str,
    magnitude: f64,
    label: &str,
    value: u64,
) -> NormalizedEvent {
    NormalizedEvent {
        v: SCHEMA_VERSION,
        timestamp: Some(timestamp),
        category: category.to_owned(),
        origin: Some("host".to_owned()),
        target: None,
        magnitude,
        direction: Direction::Neutral,
        labels: BTreeMap::from([
            ("synesthesia.lane".to_owned(), category.to_owned()),
            (label.to_owned(), value.to_string()),
        ]),
    }
}

fn identity_for(
    anonymize: bool,
    session_seed: u64,
    pid: u32,
    start_ticks: u64,
    comm: &str,
) -> String {
    if anonymize {
        let material = format!("{session_seed}\u{1f}{pid}\u{1f}{start_ticks}");
        format!("proc:{:016x}", stable_hash(material.as_bytes()))
    } else {
        format!("proc:{}[{}]@{}", bounded_comm(comm), pid, start_ticks)
    }
}

fn bounded_comm(comm: &str) -> String {
    comm.chars()
        .filter(|character| !character.is_control())
        .take(MAX_COMM_CHARS)
        .collect()
}

fn monotonic_delta(previous: u64, current: u64, resets: &mut u64) -> u64 {
    if current >= previous {
        current - previous
    } else {
        *resets = resets.saturating_add(1);
        0
    }
}

fn optional_delta(previous: Option<u64>, current: Option<u64>, resets: &mut u64) -> Option<u64> {
    match (previous, current) {
        (Some(previous), Some(current)) => Some(monotonic_delta(previous, current, resets)),
        _ => None,
    }
}

pub fn parse_proc_stat(input: &str) -> Result<ProcStat, ProcParseError> {
    let cpu = input
        .lines()
        .find(|line| line.starts_with("cpu "))
        .ok_or(ProcParseError::MissingField("cpu"))?;
    let mut fields = cpu.split_ascii_whitespace().skip(1);
    let values: Vec<u64> = fields.by_ref().map(parse_u64).collect::<Result<_, _>>()?;
    if values.len() < 4 {
        return Err(ProcParseError::MissingField("cpu counters"));
    }
    let total_cpu_ticks = values.iter().try_fold(0_u64, |sum, value| {
        sum.checked_add(*value).ok_or(ProcParseError::Overflow)
    })?;
    let idle_cpu_ticks = values[3].saturating_add(values.get(4).copied().unwrap_or(0));
    let procs_running = input
        .lines()
        .find_map(|line| line.strip_prefix("procs_running "))
        .ok_or(ProcParseError::MissingField("procs_running"))?
        .parse()
        .map_err(|_| ProcParseError::InvalidNumber)?;
    Ok(ProcStat {
        total_cpu_ticks,
        idle_cpu_ticks,
        procs_running,
    })
}

pub fn parse_process_stat(input: &str) -> Result<ProcessSample, ProcParseError> {
    let open = input
        .find('(')
        .ok_or(ProcParseError::MalformedProcessStat)?;
    let close = input
        .rfind(')')
        .filter(|close| *close > open)
        .ok_or(ProcParseError::MalformedProcessStat)?;
    let pid = input[..open]
        .trim()
        .parse()
        .map_err(|_| ProcParseError::InvalidNumber)?;
    let comm = bounded_comm(&input[open + 1..close]);
    let tail: Vec<&str> = input[close + 1..].split_ascii_whitespace().collect();
    if tail.len() <= 19 {
        return Err(ProcParseError::MissingField("process stat counters"));
    }
    let user_ticks = parse_u64(tail[11])?;
    let system_ticks = parse_u64(tail[12])?;
    let cpu_ticks = user_ticks
        .checked_add(system_ticks)
        .ok_or(ProcParseError::Overflow)?;
    let start_ticks = parse_u64(tail[19])?;
    Ok(ProcessSample {
        pid,
        comm,
        cpu_ticks,
        start_ticks,
        read_bytes: None,
        write_bytes: None,
    })
}

pub fn parse_process_io(input: &str) -> Result<(u64, u64), ProcParseError> {
    let fields: BTreeMap<&str, u64> = input
        .lines()
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| Ok((name.trim(), parse_u64(value.trim())?)))
        .collect::<Result<_, ProcParseError>>()?;
    Ok((
        *fields
            .get("read_bytes")
            .ok_or(ProcParseError::MissingField("read_bytes"))?,
        *fields
            .get("write_bytes")
            .ok_or(ProcParseError::MissingField("write_bytes"))?,
    ))
}

pub fn parse_meminfo(input: &str) -> Result<MemorySample, ProcParseError> {
    let fields: BTreeMap<&str, u64> = input
        .lines()
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| {
            let number = value
                .split_ascii_whitespace()
                .next()
                .ok_or(ProcParseError::InvalidNumber)?;
            Ok((name, parse_u64(number)?))
        })
        .collect::<Result<_, ProcParseError>>()?;
    Ok(MemorySample {
        total_kib: *fields
            .get("MemTotal")
            .ok_or(ProcParseError::MissingField("MemTotal"))?,
        available_kib: *fields
            .get("MemAvailable")
            .ok_or(ProcParseError::MissingField("MemAvailable"))?,
    })
}

fn parse_u64(value: &str) -> Result<u64, ProcParseError> {
    value.parse().map_err(|_| ProcParseError::InvalidNumber)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ProcParseError {
    #[error("missing /proc field {0}")]
    MissingField(&'static str),
    #[error("invalid number in /proc record")]
    InvalidNumber,
    #[error("counter overflow in /proc record")]
    Overflow,
    #[error("malformed /proc/<pid>/stat record")]
    MalformedProcessStat,
}

#[cfg(target_os = "linux")]
pub struct LiveProcSource {
    root: std::path::PathBuf,
    pid: Option<u32>,
    tracker: ProcTracker,
}

#[cfg(target_os = "linux")]
impl LiveProcSource {
    pub fn new(pid: Option<u32>, anonymize: bool, session_seed: u64) -> Self {
        Self {
            root: std::path::PathBuf::from("/proc"),
            pid,
            tracker: ProcTracker::new(anonymize, session_seed),
        }
    }

    #[cfg(test)]
    fn with_root(
        root: std::path::PathBuf,
        pid: Option<u32>,
        anonymize: bool,
        session_seed: u64,
    ) -> Self {
        Self {
            root,
            pid,
            tracker: ProcTracker::new(anonymize, session_seed),
        }
    }

    pub fn sample(&mut self, timestamp: f64) -> Result<Vec<NormalizedEvent>, ProcSourceError> {
        let mut sample = ProcSample::new(timestamp);
        let stat_path = self.root.join("stat");
        sample.stat = parse_proc_stat(&std::fs::read_to_string(&stat_path).map_err(|source| {
            ProcSourceError::ReadHost {
                path: stat_path,
                source,
            }
        })?)?;
        let memory_path = self.root.join("meminfo");
        match std::fs::read_to_string(&memory_path) {
            Ok(input) => match parse_meminfo(&input) {
                Ok(memory) => sample.memory = Some(memory),
                Err(_) => sample.unreadable = sample.unreadable.saturating_add(1),
            },
            Err(_) => sample.unreadable = sample.unreadable.saturating_add(1),
        }

        if let Some(pid) = self.pid {
            read_process(&self.root, pid, &mut sample);
        } else {
            let entries =
                std::fs::read_dir(&self.root).map_err(|source| ProcSourceError::ReadHost {
                    path: self.root.clone(),
                    source,
                })?;
            for entry in entries {
                let Ok(entry) = entry else {
                    sample.unreadable = sample.unreadable.saturating_add(1);
                    continue;
                };
                let Some(pid) = entry
                    .file_name()
                    .to_str()
                    .and_then(|name| name.parse::<u32>().ok())
                else {
                    continue;
                };
                read_process(&self.root, pid, &mut sample);
            }
        }
        Ok(self.tracker.ingest(sample))
    }

    pub fn stats(&self) -> ProcStats {
        self.tracker.stats()
    }
}

#[cfg(target_os = "linux")]
fn read_process(root: &std::path::Path, pid: u32, sample: &mut ProcSample) {
    if sample.processes.len() == MAX_SCANNED_PROCESSES {
        sample.scan_refused = sample.scan_refused.saturating_add(1);
        return;
    }
    let process_root = root.join(pid.to_string());
    let stat_path = process_root.join("stat");
    let stat = match std::fs::read_to_string(&stat_path) {
        Ok(input) => match parse_process_stat(&input) {
            Ok(stat) => stat,
            Err(_) => {
                sample.unreadable = sample.unreadable.saturating_add(1);
                return;
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            sample.vanished = sample.vanished.saturating_add(1);
            return;
        }
        Err(_) => {
            sample.unreadable = sample.unreadable.saturating_add(1);
            return;
        }
    };
    let mut process = stat;
    let io_path = process_root.join("io");
    match std::fs::read_to_string(io_path) {
        Ok(input) => match parse_process_io(&input) {
            Ok((read_bytes, write_bytes)) => {
                process.read_bytes = Some(read_bytes);
                process.write_bytes = Some(write_bytes);
            }
            Err(_) => sample.unreadable = sample.unreadable.saturating_add(1),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            sample.vanished = sample.vanished.saturating_add(1);
        }
        Err(_) => sample.unreadable = sample.unreadable.saturating_add(1),
    }
    sample.push_process(process);
}

#[derive(Debug, thiserror::Error)]
pub enum ProcSourceError {
    #[error("the proc activity source is supported only on Linux")]
    UnsupportedOperatingSystem,
    #[error("could not read Linux procfs path {path}: {source}")]
    ReadHost {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not parse Linux procfs host record: {0}")]
    Parse(#[from] ProcParseError),
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    fn process(pid: u32, comm: &str, start: u64, cpu: u64, read: u64, write: u64) -> ProcessSample {
        ProcessSample {
            pid,
            comm: comm.to_owned(),
            cpu_ticks: cpu,
            start_ticks: start,
            read_bytes: Some(read),
            write_bytes: Some(write),
        }
    }

    fn sample(timestamp: f64, processes: Vec<ProcessSample>) -> ProcSample {
        ProcSample {
            timestamp,
            processes,
            stat: ProcStat {
                total_cpu_ticks: 10_000,
                idle_cpu_ticks: 8_000,
                procs_running: 2,
            },
            memory: Some(MemorySample {
                total_kib: 1_000_000,
                available_kib: 500_000,
            }),
            ..ProcSample::default()
        }
    }

    #[test]
    fn parses_host_and_process_stats_including_hostile_comm_shape() {
        let stat =
            parse_proc_stat("cpu  10 2 3 40 5 6 7 8 9 10\nintr 1\nprocs_running 3\n").unwrap();
        assert_eq!(stat.total_cpu_ticks, 100);
        assert_eq!(stat.idle_cpu_ticks, 45);
        assert_eq!(stat.procs_running, 3);

        let process = parse_process_stat(
            "42 (compiler (worker) one) R 1 2 3 4 5 6 7 8 9 10 13 17 14 15 16 17 18 19 1234 21",
        )
        .unwrap();
        assert_eq!(process.pid, 42);
        assert_eq!(process.comm, "compiler (worker) one");
        assert_eq!(process.cpu_ticks, 30);
        assert_eq!(process.start_ticks, 1234);
    }

    #[test]
    fn parses_io_and_memory_without_accepting_partial_records() {
        assert_eq!(
            parse_process_io("rchar: 9\nwchar: 10\nread_bytes: 4096\nwrite_bytes: 8192\n").unwrap(),
            (4096, 8192)
        );
        assert!(parse_process_io("read_bytes: 1\n").is_err());
        assert_eq!(
            parse_meminfo("MemTotal: 1000 kB\nMemAvailable: 240 kB\n").unwrap(),
            MemorySample {
                total_kib: 1000,
                available_kib: 240
            }
        );
    }

    #[test]
    fn deltas_map_to_cpu_and_directional_io_events() {
        let mut tracker = ProcTracker::new(false, 7);
        tracker.ingest(sample(
            0.0,
            vec![process(10, "database", 100, 20, 1_000, 2_000)],
        ));
        let events = tracker.ingest(sample(
            0.25,
            vec![process(10, "database", 100, 35, 5_096, 10_192)],
        ));
        let categories: BTreeSet<_> = events.iter().map(|event| event.category.as_str()).collect();
        assert_eq!(
            categories,
            BTreeSet::from(["proc.cpu", "proc.io.read", "proc.io.write"])
        );
        assert_eq!(
            events
                .iter()
                .find(|event| event.category == "proc.cpu")
                .unwrap()
                .magnitude,
            1_920.0
        );
        assert_eq!(
            events
                .iter()
                .find(|event| event.category == "proc.io.read")
                .unwrap()
                .direction,
            Direction::Inbound
        );
        assert_eq!(
            events
                .iter()
                .find(|event| event.category == "proc.io.write")
                .unwrap()
                .direction,
            Direction::Outbound
        );
    }

    #[test]
    fn pid_reuse_emits_exit_and_start_without_counter_spike() {
        let mut tracker = ProcTracker::new(false, 11);
        tracker.ingest(sample(
            0.0,
            vec![process(99, "old-worker", 100, 1000, 1000, 1000)],
        ));
        let events = tracker.ingest(sample(1.0, vec![process(99, "new-worker", 900, 2, 0, 0)]));
        assert_eq!(
            events
                .iter()
                .map(|event| event.category.as_str())
                .collect::<Vec<_>>(),
            ["proc.exit", "proc.start"]
        );
        assert_eq!(tracker.stats().counter_resets, 0);
    }

    #[test]
    fn process_disappearance_emits_one_exit_and_is_forgotten() {
        let mut tracker = ProcTracker::new(false, 1);
        tracker.ingest(sample(0.0, vec![process(2, "worker", 50, 1, 0, 0)]));
        let first = tracker.ingest(sample(0.5, Vec::new()));
        let second = tracker.ingest(sample(1.0, Vec::new()));
        assert_eq!(
            first
                .iter()
                .filter(|event| event.category == "proc.exit")
                .count(),
            1
        );
        assert!(second.iter().all(|event| event.category != "proc.exit"));
        assert_eq!(tracker.stats().tracked_processes, 0);
    }

    #[test]
    fn initial_snapshot_is_a_quiet_baseline_and_later_birth_is_visible() {
        let mut tracker = ProcTracker::new(false, 2);
        let baseline = sample(0.0, vec![process(1, "existing", 10, 1, 0, 0)]);
        assert!(tracker.ingest(baseline).is_empty());
        let events = tracker.ingest(sample(
            0.25,
            vec![
                process(1, "existing", 10, 1, 0, 0),
                process(2, "new-worker", 20, 0, 0, 0),
            ],
        ));
        let start = events
            .iter()
            .find(|event| event.category == "proc.start")
            .unwrap();
        assert!(start.target.as_deref().unwrap().contains("new-worker"));
        assert_eq!(tracker.stats().starts, 1);
    }

    #[test]
    fn permission_and_disappearance_counts_are_separate_from_parse_failures() {
        let mut tracker = ProcTracker::new(false, 3);
        let mut baseline = sample(0.0, Vec::new());
        baseline.unreadable = 4;
        baseline.vanished = 2;
        tracker.ingest(baseline);
        let stats = tracker.stats();
        assert_eq!(stats.unreadable, 4);
        assert_eq!(stats.vanished, 2);
        assert_eq!(stats.counter_resets, 0);
    }

    #[test]
    fn unreadable_io_and_counter_reset_degrade_without_false_delta() {
        let mut tracker = ProcTracker::new(false, 3);
        let mut initial = process(3, "worker", 1, 100, 900, 900);
        initial.read_bytes = None;
        initial.write_bytes = None;
        tracker.ingest(sample(0.0, vec![initial]));
        let mut next = process(3, "worker", 1, 50, 100, 100);
        next.read_bytes = None;
        next.write_bytes = None;
        let events = tracker.ingest(sample(0.25, vec![next]));
        assert!(
            events
                .iter()
                .all(|event| !event.category.starts_with("proc.io"))
        );
        assert!(events.iter().all(|event| event.category != "proc.cpu"));
        assert_eq!(tracker.stats().counter_resets, 1);
    }

    #[test]
    fn process_tracking_and_event_output_are_bounded_deterministically() {
        let mut first = ProcTracker::new(true, 55);
        let processes: Vec<_> = (1..=(MAX_TRACKED_PROCESSES as u32 + 32))
            .map(|pid| process(pid, "worker", u64::from(pid), 0, 0, 0))
            .collect();
        assert!(first.ingest(sample(0.0, processes.clone())).is_empty());
        assert_eq!(first.stats().tracked_processes, MAX_TRACKED_PROCESSES);
        assert_eq!(first.stats().tracking_refused, 32);
        let events = first.ingest(sample(0.25, processes.clone()));
        assert!(events.len() <= MAX_EVENTS_PER_SAMPLE);
        assert!(
            events
                .iter()
                .any(|event| event.category == "proc.background")
        );

        let mut second = ProcTracker::new(true, 55);
        second.ingest(sample(0.0, processes.clone()));
        assert_eq!(events, second.ingest(sample(0.25, processes)));
    }

    #[test]
    fn anonymization_is_stable_within_session_and_removes_name_and_pid_labels() {
        let mut tracker = ProcTracker::new(true, 99);
        tracker.ingest(sample(0.0, Vec::new()));
        let start = tracker.ingest(sample(0.25, vec![process(7, "private-name", 77, 0, 0, 0)]));
        let cpu = tracker.ingest(sample(0.5, vec![process(7, "private-name", 77, 10, 0, 0)]));
        let start_identity = start
            .iter()
            .find(|event| event.category == "proc.start")
            .unwrap()
            .target
            .as_deref()
            .unwrap();
        let cpu_event = cpu
            .iter()
            .find(|event| event.category == "proc.cpu")
            .unwrap();
        assert_eq!(cpu_event.origin.as_deref(), Some(start_identity));
        assert!(!cpu_event.labels.contains_key("comm"));
        assert!(!cpu_event.labels.contains_key("pid"));
        assert!(
            !serde_json::to_string(cpu_event)
                .unwrap()
                .contains("private-name")
        );
    }

    #[test]
    fn low_memory_and_runqueue_are_observations_not_diagnoses() {
        let mut tracker = ProcTracker::new(false, 5);
        tracker.ingest(sample(0.0, Vec::new()));
        let mut pressured = sample(0.0, Vec::new());
        pressured.timestamp = 0.25;
        pressured.stat.procs_running = 8;
        pressured.memory = Some(MemorySample {
            total_kib: 1_000,
            available_kib: 90,
        });
        let events = tracker.ingest(pressured);
        assert!(events.iter().any(|event| event.category == "host.runqueue"));
        let memory = events
            .iter()
            .find(|event| event.category == "host.memory.pressure")
            .unwrap();
        assert_eq!(memory.labels["available_percent"], "9");
        assert!(!memory.category.contains("oom"));
    }

    #[test]
    fn sanitized_fixture_covers_cpu_io_churn_and_settle_without_host_identity() {
        let mut categories = BTreeSet::new();
        let mut previous = 0.0;
        for line in include_str!("../../tests/fixtures/proc-activity.ndjson").lines() {
            let event: NormalizedEvent = serde_json::from_str(line).unwrap();
            let timestamp = event.timestamp.unwrap();
            assert!(timestamp >= previous);
            previous = timestamp;
            let encoded = serde_json::to_string(&event).unwrap();
            categories.insert(event.category);
            assert!(!encoded.contains("/home/"));
            assert!(!encoded.contains("sushi-k"));
        }
        assert_eq!(
            categories,
            BTreeSet::from([
                "host.runqueue".to_owned(),
                "proc.cpu".to_owned(),
                "proc.exit".to_owned(),
                "proc.io.read".to_owned(),
                "proc.io.write".to_owned(),
                "proc.start".to_owned(),
            ])
        );
        assert_eq!(previous, 5.0);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn live_reader_boundary_uses_injected_proc_root_not_the_host() {
        use std::{
            fs,
            time::{SystemTime, UNIX_EPOCH},
        };

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("synesthesia-proc-fixture-{unique}"));
        let process_root = root.join("42");
        fs::create_dir_all(&process_root).unwrap();
        fs::write(
            root.join("stat"),
            "cpu  10 2 3 40 5 6 7 8 9 10\nprocs_running 1\n",
        )
        .unwrap();
        fs::write(
            root.join("meminfo"),
            "MemTotal: 1000 kB\nMemAvailable: 600 kB\n",
        )
        .unwrap();
        fs::write(
            process_root.join("stat"),
            "42 (compiler worker) R 1 2 3 4 5 6 7 8 9 10 13 17 14 15 16 17 18 19 1234 21",
        )
        .unwrap();
        fs::write(
            process_root.join("io"),
            "read_bytes: 4096\nwrite_bytes: 8192\n",
        )
        .unwrap();

        let mut source = LiveProcSource::with_root(root.clone(), Some(42), false, 1);
        assert!(source.sample(0.0).unwrap().is_empty());
        fs::write(
            process_root.join("stat"),
            "42 (compiler worker) R 1 2 3 4 5 6 7 8 9 10 23 27 14 15 16 17 18 19 1234 21",
        )
        .unwrap();
        fs::write(
            process_root.join("io"),
            "read_bytes: 8192\nwrite_bytes: 16384\n",
        )
        .unwrap();
        let events = source.sample(0.25).unwrap();
        assert_eq!(
            events
                .iter()
                .map(|event| event.category.as_str())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["proc.cpu", "proc.io.read", "proc.io.write"])
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn live_reader_classifies_missing_host_procfs_without_falling_back() {
        let missing = std::env::temp_dir().join("synesthesia-proc-definitely-missing");
        let mut source = LiveProcSource::with_root(missing, None, false, 1);
        assert!(matches!(
            source.sample(0.0),
            Err(ProcSourceError::ReadHost { .. })
        ));
    }

    #[test]
    fn unsupported_platform_error_is_explicit() {
        assert_eq!(
            ProcSourceError::UnsupportedOperatingSystem.to_string(),
            "the proc activity source is supported only on Linux"
        );
    }
}
