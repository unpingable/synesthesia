use anyhow::Result;

use crate::event::NormalizedEvent;

pub mod demo;
pub mod lines;
pub mod ndjson;

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
