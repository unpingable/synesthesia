use std::io::BufRead;

use anyhow::Result;

use crate::event::NormalizedEvent;

pub mod demo;
pub mod lines;
pub mod ndjson;
pub mod scheduler;
pub mod scheduler_helper;
pub mod scheduler_ipc;
#[cfg(all(target_os = "linux", feature = "ebpf"))]
pub mod scheduler_live;
pub mod tcp;
pub mod tshark;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SourceStats {
    pub accepted: u64,
    pub malformed: u64,
}

pub trait EventSource {
    fn next_event(&mut self) -> Result<Option<NormalizedEvent>>;
    fn stats(&self) -> SourceStats;
}

pub fn stable_hash(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    bytes.iter().fold(OFFSET, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(PRIME)
    })
}

pub const MAX_RECORD_BYTES: usize = 64 * 1024;

pub fn read_bounded_record<R: BufRead>(
    reader: &mut R,
    buffer: &mut Vec<u8>,
) -> std::io::Result<(usize, bool)> {
    buffer.clear();
    let mut total = 0_usize;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok((total, total > MAX_RECORD_BYTES));
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map_or(available.len(), |position| position + 1);
        let remaining = MAX_RECORD_BYTES.saturating_sub(buffer.len());
        let copied = consumed.min(remaining);
        buffer.extend_from_slice(&available[..copied]);
        reader.consume(consumed);
        total = total.saturating_add(consumed);
        if newline.is_some() {
            return Ok((total, total > MAX_RECORD_BYTES));
        }
    }
}
