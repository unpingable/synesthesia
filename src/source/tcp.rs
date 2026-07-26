use std::{
    collections::BTreeMap,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
};

use thiserror::Error;

use crate::event::{Direction, NormalizedEvent, SCHEMA_VERSION};

pub const TCP_KERNEL_EVENT_VERSION: u16 = 1;
pub const TCP_KERNEL_EVENT_BYTES: usize = 56;
pub const AF_INET: u8 = 2;
pub const AF_INET6: u8 = 10;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum TcpPathologyKind {
    Retransmit = 1,
    ResetSent = 2,
    ResetReceived = 3,
}

impl TryFrom<u8> for TcpPathologyKind {
    type Error = TcpDecodeError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Retransmit),
            2 => Ok(Self::ResetSent),
            3 => Ok(Self::ResetReceived),
            _ => Err(TcpDecodeError::UnsupportedKind(value)),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TcpAddressFamily {
    Ipv4,
    Ipv6,
}

impl TcpAddressFamily {
    fn decode(value: u8) -> Result<Self, TcpDecodeError> {
        match value {
            AF_INET => Ok(Self::Ipv4),
            AF_INET6 => Ok(Self::Ipv6),
            _ => Err(TcpDecodeError::UnsupportedFamily(value)),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Ipv4 => "ipv4",
            Self::Ipv6 => "ipv6",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KernelTcpEvent {
    pub timestamp_ns: u64,
    pub kind: TcpPathologyKind,
    pub family: TcpAddressFamily,
    pub cpu: u32,
    pub source_port: u16,
    pub destination_port: u16,
    pub socket_state: u32,
    pub source_address: [u8; 16],
    pub destination_address: [u8; 16],
}

impl KernelTcpEvent {
    pub fn decode(bytes: &[u8]) -> Result<Self, TcpDecodeError> {
        if bytes.len() != TCP_KERNEL_EVENT_BYTES {
            return Err(TcpDecodeError::WrongSize {
                expected: TCP_KERNEL_EVENT_BYTES,
                actual: bytes.len(),
            });
        }
        let version = read_u16(bytes, 8);
        if version != TCP_KERNEL_EVENT_VERSION {
            return Err(TcpDecodeError::UnsupportedVersion(version));
        }
        let mut source_address = [0; 16];
        source_address.copy_from_slice(&bytes[24..40]);
        let mut destination_address = [0; 16];
        destination_address.copy_from_slice(&bytes[40..56]);
        Ok(Self {
            timestamp_ns: read_u64(bytes, 0),
            kind: TcpPathologyKind::try_from(bytes[10])?,
            family: TcpAddressFamily::decode(bytes[11])?,
            cpu: read_u32(bytes, 12),
            // Kernel TCP tracepoints store these fields after ntohs(), so the
            // fixed native-endian kernel ABI does not need another byte swap.
            source_port: read_u16(bytes, 16),
            destination_port: read_u16(bytes, 18),
            socket_state: read_u32(bytes, 20),
            source_address,
            destination_address,
        })
    }

    pub fn source_ip(self) -> IpAddr {
        decode_ip(self.family, self.source_address)
    }

    pub fn destination_ip(self) -> IpAddr {
        decode_ip(self.family, self.destination_address)
    }

    pub fn normalize(self) -> NormalizedEvent {
        let local = endpoint(self.source_ip(), self.source_port);
        let peer = endpoint(self.destination_ip(), self.destination_port);
        let lane = stable_flow_lane(&local, &peer);
        let (category, origin, target, direction, magnitude) = match self.kind {
            TcpPathologyKind::Retransmit => {
                ("tcp.retransmit", local, peer, Direction::Outbound, 4_096.0)
            }
            TcpPathologyKind::ResetSent => {
                ("tcp.reset.send", local, peer, Direction::Outbound, 12_288.0)
            }
            TcpPathologyKind::ResetReceived => (
                "tcp.reset.receive",
                peer,
                local,
                Direction::Inbound,
                14_336.0,
            ),
        };
        NormalizedEvent {
            v: SCHEMA_VERSION,
            timestamp: Some(self.timestamp_ns as f64 / 1_000_000_000.0),
            category: category.to_owned(),
            origin: Some(origin),
            target: Some(target),
            magnitude,
            direction,
            labels: BTreeMap::from([
                ("address_family".to_owned(), self.family.label().to_owned()),
                ("cpu".to_owned(), self.cpu.to_string()),
                ("local_port".to_owned(), self.source_port.to_string()),
                ("peer_port".to_owned(), self.destination_port.to_string()),
                ("socket_state".to_owned(), self.socket_state.to_string()),
                ("synesthesia.lane".to_owned(), lane),
                ("tcp_pathology".to_owned(), category.to_owned()),
            ]),
        }
    }
}

fn stable_flow_lane(left: &str, right: &str) -> String {
    if left <= right {
        format!("tcp:{left}<->{right}")
    } else {
        format!("tcp:{right}<->{left}")
    }
}

fn decode_ip(family: TcpAddressFamily, bytes: [u8; 16]) -> IpAddr {
    match family {
        TcpAddressFamily::Ipv4 => IpAddr::V4(Ipv4Addr::new(bytes[0], bytes[1], bytes[2], bytes[3])),
        TcpAddressFamily::Ipv6 => IpAddr::V6(Ipv6Addr::from(bytes)),
    }
}

fn endpoint(address: IpAddr, port: u16) -> String {
    SocketAddr::new(address, port).to_string()
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum TcpDecodeError {
    #[error("TCP pathology event has {actual} bytes; expected exactly {expected}")]
    WrongSize { expected: usize, actual: usize },
    #[error(
        "unsupported TCP pathology event version {0}; supported kernel event version is {TCP_KERNEL_EVENT_VERSION}"
    )]
    UnsupportedVersion(u16),
    #[error("unsupported TCP pathology event kind {0}")]
    UnsupportedKind(u8),
    #[error("unsupported TCP address family {0}")]
    UnsupportedFamily(u8),
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum TcpSourceError {
    #[error("the eBPF TCP pathology source is supported only on Linux")]
    UnsupportedOperatingSystem,
    #[error("the eBPF TCP pathology source is not compiled in; rebuild with --features ebpf")]
    FeatureDisabled,
    #[error("unsupported architecture {0}; this build currently supports x86_64")]
    UnsupportedArchitecture(String),
    #[error("kernel BTF is unavailable at /sys/kernel/btf/vmlinux")]
    MissingBtf,
    #[error("required TCP tracepoint is unavailable: {0}")]
    TracepointUnavailable(String),
    #[error("insufficient eBPF permissions: {0}")]
    InsufficientPermissions(String),
    #[error("the kernel eBPF verifier rejected the TCP pathology program: {0}")]
    VerifierRejected(String),
    #[error("could not create or read the TCP eBPF ring buffer: {0}")]
    RingBufferSetup(String),
    #[error("missing external eBPF build dependency: {0}")]
    MissingDevelopmentDependency(String),
    #[error("could not load the eBPF TCP pathology source: {0}")]
    Load(String),
}

impl TcpSourceError {
    pub fn classify_load_message(message: &str) -> Self {
        let lower = message.to_ascii_lowercase();
        if lower.contains("permission denied")
            || lower.contains("operation not permitted")
            || lower.contains("eperm")
            || lower.contains("insufficient ebpf permissions")
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

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_ne_bytes(bytes[offset..offset + 2].try_into().expect("checked size"))
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_ne_bytes(bytes[offset..offset + 4].try_into().expect("checked size"))
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_ne_bytes(bytes[offset..offset + 8].try_into().expect("checked size"))
}

#[cfg(test)]
mod tests {
    use crate::{model::TemporalModel, source::stable_hash};

    use super::*;

    fn encoded(event: KernelTcpEvent) -> [u8; TCP_KERNEL_EVENT_BYTES] {
        let mut bytes = [0; TCP_KERNEL_EVENT_BYTES];
        bytes[0..8].copy_from_slice(&event.timestamp_ns.to_ne_bytes());
        bytes[8..10].copy_from_slice(&TCP_KERNEL_EVENT_VERSION.to_ne_bytes());
        bytes[10] = event.kind as u8;
        bytes[11] = match event.family {
            TcpAddressFamily::Ipv4 => AF_INET,
            TcpAddressFamily::Ipv6 => AF_INET6,
        };
        bytes[12..16].copy_from_slice(&event.cpu.to_ne_bytes());
        bytes[16..18].copy_from_slice(&event.source_port.to_ne_bytes());
        bytes[18..20].copy_from_slice(&event.destination_port.to_ne_bytes());
        bytes[20..24].copy_from_slice(&event.socket_state.to_ne_bytes());
        bytes[24..40].copy_from_slice(&event.source_address);
        bytes[40..56].copy_from_slice(&event.destination_address);
        bytes
    }

    fn ipv4(kind: TcpPathologyKind) -> KernelTcpEvent {
        let mut source = [0; 16];
        source[..4].copy_from_slice(&[192, 0, 2, 10]);
        let mut destination = [0; 16];
        destination[..4].copy_from_slice(&[198, 51, 100, 20]);
        KernelTcpEvent {
            timestamp_ns: 9_000_000,
            kind,
            family: TcpAddressFamily::Ipv4,
            cpu: 3,
            source_port: 42_000,
            destination_port: 443,
            socket_state: 1,
            source_address: source,
            destination_address: destination,
        }
    }

    #[test]
    fn fixed_kernel_layout_is_exactly_56_bytes() {
        assert_eq!(TCP_KERNEL_EVENT_BYTES, 56);
        let expected = ipv4(TcpPathologyKind::Retransmit);
        assert_eq!(KernelTcpEvent::decode(&encoded(expected)), Ok(expected));
    }

    #[test]
    fn refuses_wrong_size_version_kind_and_family() {
        assert!(matches!(
            KernelTcpEvent::decode(&[0; 55]),
            Err(TcpDecodeError::WrongSize { .. })
        ));
        let mut bytes = encoded(ipv4(TcpPathologyKind::Retransmit));
        bytes[8..10].copy_from_slice(&2_u16.to_ne_bytes());
        assert_eq!(
            KernelTcpEvent::decode(&bytes),
            Err(TcpDecodeError::UnsupportedVersion(2))
        );
        bytes = encoded(ipv4(TcpPathologyKind::Retransmit));
        bytes[10] = 99;
        assert_eq!(
            KernelTcpEvent::decode(&bytes),
            Err(TcpDecodeError::UnsupportedKind(99))
        );
        bytes = encoded(ipv4(TcpPathologyKind::Retransmit));
        bytes[11] = 1;
        assert_eq!(
            KernelTcpEvent::decode(&bytes),
            Err(TcpDecodeError::UnsupportedFamily(1))
        );
    }

    #[test]
    fn decodes_ipv4_and_host_order_ports() {
        let event = KernelTcpEvent::decode(&encoded(ipv4(TcpPathologyKind::Retransmit))).unwrap();
        assert_eq!(event.source_ip(), "192.0.2.10".parse::<IpAddr>().unwrap());
        assert_eq!(
            event.destination_ip(),
            "198.51.100.20".parse::<IpAddr>().unwrap()
        );
        assert_eq!(event.source_port, 42_000);
        assert_eq!(event.destination_port, 443);
    }

    #[test]
    fn decodes_ipv6_addresses_and_formats_bracketed_endpoints() {
        let event = KernelTcpEvent {
            family: TcpAddressFamily::Ipv6,
            source_address: "2001:db8::10".parse::<Ipv6Addr>().unwrap().octets(),
            destination_address: "2001:db8:1::20".parse::<Ipv6Addr>().unwrap().octets(),
            ..ipv4(TcpPathologyKind::Retransmit)
        };
        let decoded = KernelTcpEvent::decode(&encoded(event)).unwrap();
        assert_eq!(
            decoded.source_ip(),
            "2001:db8::10".parse::<IpAddr>().unwrap()
        );
        let normalized = decoded.normalize();
        assert_eq!(normalized.origin.as_deref(), Some("[2001:db8::10]:42000"));
        assert_eq!(normalized.target.as_deref(), Some("[2001:db8:1::20]:443"));
    }

    #[test]
    fn pathology_kinds_normalize_with_truthful_direction_and_impact() {
        let retransmit = ipv4(TcpPathologyKind::Retransmit).normalize();
        let sent = ipv4(TcpPathologyKind::ResetSent).normalize();
        let received = ipv4(TcpPathologyKind::ResetReceived).normalize();
        assert_eq!(retransmit.category, "tcp.retransmit");
        assert_eq!(retransmit.direction, Direction::Outbound);
        assert_eq!(sent.category, "tcp.reset.send");
        assert_eq!(sent.direction, Direction::Outbound);
        assert_eq!(received.category, "tcp.reset.receive");
        assert_eq!(received.direction, Direction::Inbound);
        assert_eq!(received.origin.as_deref(), Some("198.51.100.20:443"));
        assert!(received.magnitude > sent.magnitude);
        assert!(sent.magnitude > retransmit.magnitude);
    }

    #[test]
    fn repeated_flow_mapping_is_stable_and_distinct_flows_diverge() {
        let first = ipv4(TcpPathologyKind::Retransmit).normalize();
        let repeated = ipv4(TcpPathologyKind::ResetSent).normalize();
        let mut other = ipv4(TcpPathologyKind::Retransmit);
        other.destination_address[3] = 21;
        let other = other.normalize();
        let flow_hash = |event: &NormalizedEvent| stable_hash(event.flow_key().as_bytes());
        assert_ne!(first.category, repeated.category);
        assert_ne!(flow_hash(&first), flow_hash(&repeated));

        let mut model = TemporalModel::default();
        model.ingest(first, 0.0);
        model.ingest(repeated, 0.1);
        model.ingest(other, 0.2);
        let activity = model.snapshot().activity;
        assert_eq!(activity[0].lane, activity[1].lane);
        assert_ne!(activity[1].lane, activity[2].lane);
    }

    #[test]
    fn source_errors_classify_permission_and_verifier_failures() {
        assert!(matches!(
            TcpSourceError::classify_load_message("Operation not permitted"),
            TcpSourceError::InsufficientPermissions(_)
        ));
        assert!(matches!(
            TcpSourceError::classify_load_message("verifier rejected instruction"),
            TcpSourceError::VerifierRejected(_)
        ));
    }

    #[test]
    fn raw_fixture_covers_all_pathology_kinds_and_families() {
        let fixture = include_str!("../../tests/fixtures/tcp-kernel-events.tsv");
        let mut decoded = Vec::new();
        for line in fixture.lines().filter(|line| !line.starts_with('#')) {
            let fields: Vec<_> = line.split('\t').collect();
            assert_eq!(fields.len(), 10);
            let mut bytes = [0; TCP_KERNEL_EVENT_BYTES];
            bytes[0..8].copy_from_slice(&1_000_000_u64.to_ne_bytes());
            bytes[8..10].copy_from_slice(&fields[0].parse::<u16>().unwrap().to_ne_bytes());
            bytes[10] = fields[1].parse().unwrap();
            bytes[11] = fields[2].parse().unwrap();
            bytes[12..16].copy_from_slice(&fields[3].parse::<u32>().unwrap().to_ne_bytes());
            bytes[16..18].copy_from_slice(&fields[4].parse::<u16>().unwrap().to_ne_bytes());
            bytes[18..20].copy_from_slice(&fields[5].parse::<u16>().unwrap().to_ne_bytes());
            bytes[20..24].copy_from_slice(&fields[8].parse::<u32>().unwrap().to_ne_bytes());
            bytes[24..40].copy_from_slice(&decode_hex_16(fields[6]));
            bytes[40..56].copy_from_slice(&decode_hex_16(fields[7]));
            decoded.push((KernelTcpEvent::decode(&bytes).unwrap(), fields[9]));
        }
        assert_eq!(decoded.len(), 4);
        assert_eq!(decoded[0].0.source_ip().to_string(), "192.0.2.10");
        assert_eq!(decoded[3].0.source_ip().to_string(), "2001:db8::10");
        assert_eq!(
            decoded.iter().map(|(_, label)| *label).collect::<Vec<_>>(),
            [
                "retransmit",
                "reset_sent",
                "reset_received",
                "retransmit_ipv6"
            ]
        );
    }

    fn decode_hex_16(value: &str) -> [u8; 16] {
        assert_eq!(value.len(), 32);
        let mut bytes = [0; 16];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).unwrap();
        }
        bytes
    }
}
