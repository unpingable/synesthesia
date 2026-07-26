use std::path::Path;

use aya::{
    Ebpf,
    maps::{MapData, PerCpuArray, RingBuf},
    programs::TracePoint,
};

use crate::source::scheduler::{KernelSchedulerEvent, SchedulerDecodeError, SchedulerSourceError};

const BYTECODE: &[u8] = aya::include_bytes_aligned!(concat!(env!("OUT_DIR"), "/scheduler.bpf.o"));
const TRACEPOINTS: [(&str, &str); 4] = [
    ("synesthesia_sched_switch", "sched_switch"),
    ("synesthesia_sched_wakeup", "sched_wakeup"),
    ("synesthesia_sched_wakeup_new", "sched_wakeup_new"),
    ("synesthesia_sched_migrate", "sched_migrate_task"),
];

pub struct LiveScheduler {
    // Aya retains attached links in each program; dropping this detaches all of them.
    _bpf: Ebpf,
    events: RingBuf<MapData>,
    losses: PerCpuArray<MapData, u64>,
}

impl LiveScheduler {
    pub fn attach() -> Result<Self, SchedulerSourceError> {
        if std::env::consts::ARCH != "x86_64" {
            return Err(SchedulerSourceError::UnsupportedArchitecture(
                std::env::consts::ARCH.to_owned(),
            ));
        }
        if !Path::new("/sys/kernel/btf/vmlinux").is_file() {
            return Err(SchedulerSourceError::MissingBtf);
        }

        let mut bpf = Ebpf::load(BYTECODE).map_err(|error| {
            SchedulerSourceError::classify_load_message(&format!("{error}: {error:?}"))
        })?;
        for (program_name, tracepoint_name) in TRACEPOINTS {
            let program: &mut TracePoint = bpf
                .program_mut(program_name)
                .ok_or_else(|| {
                    SchedulerSourceError::Load(format!(
                        "compiled program {program_name} is missing"
                    ))
                })?
                .try_into()
                .map_err(|error: aya::programs::ProgramError| {
                    SchedulerSourceError::Load(error.to_string())
                })?;
            program
                .load()
                .map_err(|error| SchedulerSourceError::classify_load_message(&error.to_string()))?;
            program
                .attach("sched", tracepoint_name)
                .map_err(|error| classify_attach_error(tracepoint_name, &error.to_string()))?;
        }

        let events = RingBuf::try_from(bpf.take_map("EVENTS").ok_or_else(|| {
            SchedulerSourceError::RingBufferSetup("EVENTS map is absent".to_owned())
        })?)
        .map_err(|error| SchedulerSourceError::RingBufferSetup(error.to_string()))?;
        let losses = PerCpuArray::try_from(bpf.take_map("LOSSES").ok_or_else(|| {
            SchedulerSourceError::RingBufferSetup("LOSSES map is absent".to_owned())
        })?)
        .map_err(|error| SchedulerSourceError::RingBufferSetup(error.to_string()))?;

        Ok(Self {
            _bpf: bpf,
            events,
            losses,
        })
    }

    pub fn next_event(&mut self) -> Option<Result<KernelSchedulerEvent, SchedulerDecodeError>> {
        self.events
            .next()
            .map(|item| KernelSchedulerEvent::decode(&item))
    }

    pub fn kernel_ring_drops(&self) -> Result<u64, SchedulerSourceError> {
        let values = self
            .losses
            .get(&0, 0)
            .map_err(|error| SchedulerSourceError::RingBufferSetup(error.to_string()))?;
        Ok(values.iter().copied().sum())
    }
}

fn classify_attach_error(tracepoint: &str, message: &str) -> SchedulerSourceError {
    let lower = message.to_ascii_lowercase();
    if lower.contains("no such file") || lower.contains("not found") {
        SchedulerSourceError::TracepointUnavailable(tracepoint.to_owned())
    } else {
        SchedulerSourceError::classify_load_message(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_tracepoint_is_classified_separately() {
        assert_eq!(
            classify_attach_error("sched_switch", "No such file or directory"),
            SchedulerSourceError::TracepointUnavailable("sched_switch".to_owned())
        );
    }

    #[test]
    fn permission_attach_failure_is_actionable() {
        assert!(matches!(
            classify_attach_error("sched_switch", "Operation not permitted"),
            SchedulerSourceError::InsufficientPermissions(_)
        ));
    }

    #[test]
    #[ignore = "requires explicit eBPF privilege and live scheduler tracepoints"]
    #[cfg(feature = "ebpf-live")]
    fn attaches_to_live_scheduler_tracepoints() {
        LiveScheduler::attach().expect("live scheduler attachment");
    }
}
