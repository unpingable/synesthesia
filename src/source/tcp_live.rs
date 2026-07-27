use std::path::Path;

use aya::{
    Ebpf,
    maps::{MapData, PerCpuArray, RingBuf},
    programs::TracePoint,
};

use crate::source::{
    ebpf_prerequisites::{SUPPORTED_ARCHITECTURE, TCP_TRACEPOINTS},
    tcp::{KernelTcpEvent, TcpDecodeError, TcpSourceError},
};

const BYTECODE: &[u8] = aya::include_bytes_aligned!(concat!(env!("OUT_DIR"), "/tcp.bpf.o"));

pub struct LiveTcp {
    // Program-owned links detach and all maps close when this value drops.
    _bpf: Ebpf,
    events: RingBuf<MapData>,
    losses: PerCpuArray<MapData, u64>,
}

impl LiveTcp {
    pub fn attach() -> Result<Self, TcpSourceError> {
        if std::env::consts::ARCH != SUPPORTED_ARCHITECTURE {
            return Err(TcpSourceError::UnsupportedArchitecture(
                std::env::consts::ARCH.to_owned(),
            ));
        }
        if !Path::new("/sys/kernel/btf/vmlinux").is_file() {
            return Err(TcpSourceError::MissingBtf);
        }

        let mut bpf = Ebpf::load(BYTECODE).map_err(|error| {
            TcpSourceError::classify_load_message(&format!("{error}: {error:?}"))
        })?;
        for tracepoint in TCP_TRACEPOINTS {
            let program: &mut TracePoint = bpf
                .program_mut(tracepoint.program)
                .ok_or_else(|| {
                    TcpSourceError::Load(format!(
                        "compiled program {} is missing",
                        tracepoint.program
                    ))
                })?
                .try_into()
                .map_err(|error: aya::programs::ProgramError| {
                    TcpSourceError::Load(error.to_string())
                })?;
            program
                .load()
                .map_err(|error| TcpSourceError::classify_load_message(&error.to_string()))?;
            program
                .attach(tracepoint.group, tracepoint.name)
                .map_err(|error| classify_attach_error(tracepoint.name, &error.to_string()))?;
        }

        let events = RingBuf::try_from(bpf.take_map("TCP_EVENTS").ok_or_else(|| {
            TcpSourceError::RingBufferSetup("TCP_EVENTS map is absent".to_owned())
        })?)
        .map_err(|error| TcpSourceError::RingBufferSetup(error.to_string()))?;
        let losses = PerCpuArray::try_from(bpf.take_map("TCP_LOSSES").ok_or_else(|| {
            TcpSourceError::RingBufferSetup("TCP_LOSSES map is absent".to_owned())
        })?)
        .map_err(|error| TcpSourceError::RingBufferSetup(error.to_string()))?;

        Ok(Self {
            _bpf: bpf,
            events,
            losses,
        })
    }

    pub fn next_event(&mut self) -> Option<Result<KernelTcpEvent, TcpDecodeError>> {
        self.events.next().map(|item| KernelTcpEvent::decode(&item))
    }

    pub fn kernel_ring_drops(&self) -> Result<u64, TcpSourceError> {
        let values = self
            .losses
            .get(&0, 0)
            .map_err(|error| TcpSourceError::RingBufferSetup(error.to_string()))?;
        Ok(values.iter().copied().sum())
    }
}

fn classify_attach_error(tracepoint: &str, message: &str) -> TcpSourceError {
    let lower = message.to_ascii_lowercase();
    if lower.contains("no such file") || lower.contains("not found") {
        TcpSourceError::TracepointUnavailable(tracepoint.to_owned())
    } else {
        TcpSourceError::classify_load_message(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_tcp_tracepoint_is_classified_separately() {
        assert_eq!(
            classify_attach_error("tcp_retransmit_skb", "No such file or directory"),
            TcpSourceError::TracepointUnavailable("tcp_retransmit_skb".to_owned())
        );
    }

    #[test]
    fn tcp_attach_permission_failure_is_actionable() {
        assert!(matches!(
            classify_attach_error("tcp_retransmit_skb", "Operation not permitted"),
            TcpSourceError::InsufficientPermissions(_)
        ));
    }

    #[test]
    #[ignore = "requires explicit eBPF privilege and live TCP tracepoints"]
    #[cfg(feature = "ebpf-live")]
    fn attaches_to_live_tcp_tracepoints() {
        LiveTcp::attach().expect("live TCP pathology attachment");
    }
}
