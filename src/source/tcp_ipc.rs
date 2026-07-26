use std::{
    collections::BTreeMap,
    io::{self, Read, Write},
};

use thiserror::Error;

use crate::source::tcp::{AF_INET, AF_INET6, KernelTcpEvent, TcpAddressFamily, TcpPathologyKind};

pub const TCP_WIRE_BYTES: usize = 96;
pub const TCP_WIRE_MAGIC: [u8; 4] = *b"SYNT";
pub const TCP_WIRE_VERSION: u16 = 1;
pub const MAX_TCP_BUCKETS_PER_WINDOW: usize = 1_024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum TcpPulseKind {
    Heartbeat = 0,
    Retransmit = 1,
    ResetSent = 2,
    ResetReceived = 3,
}

impl TryFrom<u8> for TcpPulseKind {
    type Error = TcpWireError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Heartbeat),
            1 => Ok(Self::Retransmit),
            2 => Ok(Self::ResetSent),
            3 => Ok(Self::ResetReceived),
            _ => Err(TcpWireError::UnsupportedKind(value)),
        }
    }
}

impl From<TcpPathologyKind> for TcpPulseKind {
    fn from(value: TcpPathologyKind) -> Self {
        match value {
            TcpPathologyKind::Retransmit => Self::Retransmit,
            TcpPathologyKind::ResetSent => Self::ResetSent,
            TcpPathologyKind::ResetReceived => Self::ResetReceived,
        }
    }
}

impl TcpPulseKind {
    fn pathology(self) -> Option<TcpPathologyKind> {
        match self {
            Self::Heartbeat => None,
            Self::Retransmit => Some(TcpPathologyKind::Retransmit),
            Self::ResetSent => Some(TcpPathologyKind::ResetSent),
            Self::ResetReceived => Some(TcpPathologyKind::ResetReceived),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NormalizedTcpPulse {
    pub timestamp_ns: u64,
    pub kind: TcpPulseKind,
    pub family: TcpAddressFamily,
    pub source_port: u16,
    pub destination_port: u16,
    pub event_count: u32,
    pub cpu: u32,
    pub socket_state: u32,
    pub source_address: [u8; 16],
    pub destination_address: [u8; 16],
    pub magnitude: f64,
    pub kernel_ring_drops: u64,
    pub collector_drops: u64,
    pub ipc_drops: u64,
}

impl NormalizedTcpPulse {
    pub fn write_to(&self, writer: &mut impl Write) -> io::Result<()> {
        writer.write_all(&self.encode())
    }

    pub fn read_from(reader: &mut impl Read) -> Result<Option<Self>, TcpWireError> {
        let mut bytes = [0; TCP_WIRE_BYTES];
        let mut read = 0;
        while read < bytes.len() {
            match reader.read(&mut bytes[read..]) {
                Ok(0) if read == 0 => return Ok(None),
                Ok(0) => return Err(TcpWireError::Truncated(read)),
                Ok(count) => read += count,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(TcpWireError::Io(error)),
            }
        }
        Self::decode(&bytes).map(Some)
    }

    pub fn encode(&self) -> [u8; TCP_WIRE_BYTES] {
        let mut bytes = [0; TCP_WIRE_BYTES];
        bytes[0..4].copy_from_slice(&TCP_WIRE_MAGIC);
        bytes[4..6].copy_from_slice(&TCP_WIRE_VERSION.to_le_bytes());
        bytes[6] = self.kind as u8;
        bytes[7] = family_number(self.family);
        bytes[8..16].copy_from_slice(&self.timestamp_ns.to_le_bytes());
        bytes[16..18].copy_from_slice(&self.source_port.to_le_bytes());
        bytes[18..20].copy_from_slice(&self.destination_port.to_le_bytes());
        bytes[20..24].copy_from_slice(&self.event_count.to_le_bytes());
        bytes[24..28].copy_from_slice(&self.cpu.to_le_bytes());
        bytes[28..32].copy_from_slice(&self.socket_state.to_le_bytes());
        bytes[32..48].copy_from_slice(&self.source_address);
        bytes[48..64].copy_from_slice(&self.destination_address);
        bytes[64..72].copy_from_slice(&self.magnitude.to_le_bytes());
        bytes[72..80].copy_from_slice(&self.kernel_ring_drops.to_le_bytes());
        bytes[80..88].copy_from_slice(&self.collector_drops.to_le_bytes());
        bytes[88..96].copy_from_slice(&self.ipc_drops.to_le_bytes());
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, TcpWireError> {
        if bytes.len() != TCP_WIRE_BYTES {
            return Err(TcpWireError::WrongSize(bytes.len()));
        }
        if bytes[0..4] != TCP_WIRE_MAGIC {
            return Err(TcpWireError::BadMagic);
        }
        let version = read_u16(bytes, 4);
        if version != TCP_WIRE_VERSION {
            return Err(TcpWireError::UnsupportedVersion(version));
        }
        let mut source_address = [0; 16];
        source_address.copy_from_slice(&bytes[32..48]);
        let mut destination_address = [0; 16];
        destination_address.copy_from_slice(&bytes[48..64]);
        Ok(Self {
            timestamp_ns: read_u64(bytes, 8),
            kind: TcpPulseKind::try_from(bytes[6])?,
            family: decode_family(bytes[7])?,
            source_port: read_u16(bytes, 16),
            destination_port: read_u16(bytes, 18),
            event_count: read_u32(bytes, 20),
            cpu: read_u32(bytes, 24),
            socket_state: read_u32(bytes, 28),
            source_address,
            destination_address,
            magnitude: f64::from_le_bytes(bytes[64..72].try_into().expect("checked size")),
            kernel_ring_drops: read_u64(bytes, 72),
            collector_drops: read_u64(bytes, 80),
            ipc_drops: read_u64(bytes, 88),
        })
    }

    pub fn into_normalized(self) -> Option<crate::event::NormalizedEvent> {
        let kind = self.kind.pathology()?;
        let mut event = KernelTcpEvent {
            timestamp_ns: self.timestamp_ns,
            kind,
            family: self.family,
            cpu: self.cpu,
            source_port: self.source_port,
            destination_port: self.destination_port,
            socket_state: self.socket_state,
            source_address: self.source_address,
            destination_address: self.destination_address,
        }
        .normalize();
        event.magnitude = self.magnitude;
        event
            .labels
            .insert("event_count".to_owned(), self.event_count.to_string());
        event.labels.insert(
            "synesthesia.event_count".to_owned(),
            self.event_count.to_string(),
        );
        Some(event)
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct TcpBucketKey {
    kind: TcpPathologyKind,
    family: TcpAddressFamily,
    source_port: u16,
    destination_port: u16,
    source_address: [u8; 16],
    destination_address: [u8; 16],
}

#[derive(Clone, Copy, Debug, Default)]
struct TcpBucket {
    count: u32,
    cpu: u32,
    socket_state: u32,
}

pub struct TcpAggregator {
    buckets: BTreeMap<TcpBucketKey, TcpBucket>,
    collector_drops: u64,
}

impl TcpAggregator {
    pub fn new() -> Self {
        Self {
            buckets: BTreeMap::new(),
            collector_drops: 0,
        }
    }

    pub fn ingest(&mut self, event: KernelTcpEvent) {
        let key = TcpBucketKey {
            kind: event.kind,
            family: event.family,
            source_port: event.source_port,
            destination_port: event.destination_port,
            source_address: event.source_address,
            destination_address: event.destination_address,
        };
        if !self.buckets.contains_key(&key) && self.buckets.len() >= MAX_TCP_BUCKETS_PER_WINDOW {
            self.collector_drops = self.collector_drops.saturating_add(1);
            return;
        }
        let bucket = self.buckets.entry(key).or_default();
        bucket.count = bucket.count.saturating_add(1);
        bucket.cpu = event.cpu;
        bucket.socket_state = event.socket_state;
    }

    pub fn flush(&mut self, timestamp_ns: u64, kernel_ring_drops: u64) -> Vec<NormalizedTcpPulse> {
        if self.buckets.is_empty() {
            return vec![heartbeat(
                timestamp_ns,
                kernel_ring_drops,
                self.collector_drops,
            )];
        }
        let collector_drops = self.collector_drops;
        std::mem::take(&mut self.buckets)
            .into_iter()
            .map(|(key, bucket)| {
                let magnitude = pulse_magnitude(key.kind, bucket.count);
                NormalizedTcpPulse {
                    timestamp_ns,
                    kind: key.kind.into(),
                    family: key.family,
                    source_port: key.source_port,
                    destination_port: key.destination_port,
                    event_count: bucket.count,
                    cpu: bucket.cpu,
                    socket_state: bucket.socket_state,
                    source_address: key.source_address,
                    destination_address: key.destination_address,
                    magnitude,
                    kernel_ring_drops,
                    collector_drops,
                    ipc_drops: 0,
                }
            })
            .collect()
    }

    pub fn bucket_count(&self) -> usize {
        self.buckets.len()
    }
}

impl Default for TcpAggregator {
    fn default() -> Self {
        Self::new()
    }
}

fn heartbeat(
    timestamp_ns: u64,
    kernel_ring_drops: u64,
    collector_drops: u64,
) -> NormalizedTcpPulse {
    NormalizedTcpPulse {
        timestamp_ns,
        kind: TcpPulseKind::Heartbeat,
        family: TcpAddressFamily::Ipv4,
        source_port: 0,
        destination_port: 0,
        event_count: 0,
        cpu: 0,
        socket_state: 0,
        source_address: [0; 16],
        destination_address: [0; 16],
        magnitude: 0.0,
        kernel_ring_drops,
        collector_drops,
        ipc_drops: 0,
    }
}

fn pulse_magnitude(kind: TcpPathologyKind, count: u32) -> f64 {
    let count = f64::from(count);
    match kind {
        TcpPathologyKind::Retransmit => (4_096.0 + count * 1_536.0).min(65_536.0),
        TcpPathologyKind::ResetSent => (16_384.0 + count * 8_192.0).min(131_072.0),
        TcpPathologyKind::ResetReceived => (20_480.0 + count * 8_192.0).min(131_072.0),
    }
}

#[derive(Debug, Error)]
pub enum TcpWireError {
    #[error("TCP helper record has {0} bytes; expected {TCP_WIRE_BYTES}")]
    WrongSize(usize),
    #[error("TCP helper record has invalid magic")]
    BadMagic,
    #[error("unsupported TCP helper protocol version {0}")]
    UnsupportedVersion(u16),
    #[error("unsupported TCP pulse kind {0}")]
    UnsupportedKind(u8),
    #[error("unsupported TCP pulse address family {0}")]
    UnsupportedFamily(u8),
    #[error("TCP helper stream ended after {0} bytes of a record")]
    Truncated(usize),
    #[error("TCP helper I/O failed: {0}")]
    Io(#[from] io::Error),
}

fn family_number(family: TcpAddressFamily) -> u8 {
    match family {
        TcpAddressFamily::Ipv4 => AF_INET,
        TcpAddressFamily::Ipv6 => AF_INET6,
    }
}

fn decode_family(value: u8) -> Result<TcpAddressFamily, TcpWireError> {
    match value {
        AF_INET => Ok(TcpAddressFamily::Ipv4),
        AF_INET6 => Ok(TcpAddressFamily::Ipv6),
        _ => Err(TcpWireError::UnsupportedFamily(value)),
    }
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

    fn raw(kind: TcpPathologyKind, flow: u16) -> KernelTcpEvent {
        let mut source = [0; 16];
        source[..4].copy_from_slice(&[192, 0, 2, 10]);
        let mut destination = [0; 16];
        destination[..4].copy_from_slice(&[198, 51, 100, (flow % 250) as u8 + 1]);
        KernelTcpEvent {
            timestamp_ns: 1_000_000,
            kind,
            family: TcpAddressFamily::Ipv4,
            cpu: 2,
            source_port: 40_000 + flow,
            destination_port: 443,
            socket_state: 1,
            source_address: source,
            destination_address: destination,
        }
    }

    #[test]
    fn tcp_wire_protocol_is_distinct_fixed_and_round_trips() {
        assert_eq!(TCP_WIRE_BYTES, 96);
        assert_ne!(TCP_WIRE_MAGIC, crate::source::scheduler_ipc::WIRE_MAGIC);
        let mut aggregator = TcpAggregator::new();
        aggregator.ingest(raw(TcpPathologyKind::Retransmit, 1));
        let pulse = aggregator.flush(2_000_000, 7).remove(0);
        assert_eq!(NormalizedTcpPulse::decode(&pulse.encode()).unwrap(), pulse);
        assert_eq!(pulse.kernel_ring_drops, 7);
        assert_eq!(pulse.ipc_drops, 0);
    }

    #[test]
    fn repeated_retransmits_coalesce_and_scale_magnitude() {
        let mut one = TcpAggregator::new();
        one.ingest(raw(TcpPathologyKind::Retransmit, 1));
        let one = one.flush(2_000_000, 0).remove(0);
        let mut burst = TcpAggregator::new();
        for _ in 0..20 {
            burst.ingest(raw(TcpPathologyKind::Retransmit, 1));
        }
        let burst = burst.flush(2_000_000, 0).remove(0);
        assert_eq!(burst.event_count, 20);
        assert!(burst.magnitude > one.magnitude);
        assert!(burst.magnitude <= 65_536.0);
    }

    #[test]
    fn reset_pulses_remain_distinct_from_retransmits() {
        let mut aggregator = TcpAggregator::new();
        aggregator.ingest(raw(TcpPathologyKind::Retransmit, 1));
        aggregator.ingest(raw(TcpPathologyKind::ResetSent, 1));
        aggregator.ingest(raw(TcpPathologyKind::ResetReceived, 1));
        let pulses = aggregator.flush(2_000_000, 0);
        assert_eq!(pulses.len(), 3);
        assert!(
            pulses
                .iter()
                .any(|pulse| pulse.kind == TcpPulseKind::ResetSent)
        );
        assert!(
            pulses
                .iter()
                .any(|pulse| pulse.kind == TcpPulseKind::ResetReceived)
        );
    }

    #[test]
    fn aggregation_refuses_new_buckets_deterministically_at_bound() {
        let mut aggregator = TcpAggregator::new();
        for flow in 0..(MAX_TCP_BUCKETS_PER_WINDOW as u16 + 50) {
            aggregator.ingest(raw(TcpPathologyKind::Retransmit, flow));
        }
        assert_eq!(aggregator.bucket_count(), MAX_TCP_BUCKETS_PER_WINDOW);
        let pulses = aggregator.flush(2_000_000, 0);
        assert_eq!(pulses.len(), MAX_TCP_BUCKETS_PER_WINDOW);
        assert!(pulses.iter().all(|pulse| pulse.collector_drops == 50));
    }

    #[test]
    fn malformed_or_cross_protocol_wire_records_are_refused() {
        let pulse = heartbeat(0, 0, 0);
        let mut bytes = pulse.encode();
        bytes[0..4].copy_from_slice(&crate::source::scheduler_ipc::WIRE_MAGIC);
        assert!(matches!(
            NormalizedTcpPulse::decode(&bytes),
            Err(TcpWireError::BadMagic)
        ));
        bytes = pulse.encode();
        bytes[4..6].copy_from_slice(&99_u16.to_le_bytes());
        assert!(matches!(
            NormalizedTcpPulse::decode(&bytes),
            Err(TcpWireError::UnsupportedVersion(99))
        ));
    }

    #[test]
    fn normalized_pulse_preserves_count_and_loss_boundaries() {
        let mut aggregator = TcpAggregator::new();
        for _ in 0..3 {
            aggregator.ingest(raw(TcpPathologyKind::Retransmit, 1));
        }
        let pulse = aggregator.flush(2_000_000, 11).remove(0);
        let event = pulse.into_normalized().unwrap();
        assert_eq!(event.category, "tcp.retransmit");
        assert_eq!(event.labels["synesthesia.event_count"], "3");
        assert_eq!(pulse.kernel_ring_drops, 11);
        assert_eq!(pulse.collector_drops, 0);
        assert_eq!(pulse.ipc_drops, 0);
    }

    #[test]
    fn raw_event_version_constant_matches_wire_materialization() {
        assert_eq!(crate::source::tcp::TCP_KERNEL_EVENT_VERSION, 1);
    }
}
