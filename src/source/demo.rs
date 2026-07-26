use std::collections::BTreeMap;

use crate::event::{Direction, NormalizedEvent, SCHEMA_VERSION};

pub struct DemoSource {
    rng: SplitMix64,
    index: u64,
    seed: u64,
}

impl DemoSource {
    pub fn new(seed: u64) -> Self {
        Self {
            rng: SplitMix64(seed),
            index: 0,
            seed,
        }
    }
}

impl Iterator for DemoSource {
    type Item = NormalizedEvent;

    fn next(&mut self) -> Option<Self::Item> {
        let index = self.index;
        self.index += 1;
        let phase = (index / 48) % 5;
        let burst = matches!(phase, 1 | 4) && index % 48 < 18;
        let heavy = index % 137 == 89;
        let categories = ["tcp", "udp", "dns", "tls", "pulse", "ghost"];
        let category_index = ((self.rng.next() >> 17) as usize + phase as usize) % categories.len();
        let pair = ((index / 13) + (self.rng.next() % 3)) % 11;
        let direction = match (index / 9 + pair) % 4 {
            0 => Direction::Inbound,
            1 => Direction::Outbound,
            2 => Direction::Neutral,
            _ => Direction::Unknown,
        };
        let noise = 12.0 + (self.rng.next() % 96) as f64;
        let magnitude = if heavy {
            8_192.0 + noise
        } else if burst {
            600.0 + noise * 8.0
        } else {
            noise
        };
        let mut labels = BTreeMap::new();
        labels.insert("synthetic".to_owned(), "true".to_owned());
        labels.insert("seed".to_owned(), self.seed.to_string());
        Some(NormalizedEvent {
            v: SCHEMA_VERSION,
            timestamp: Some((index * 45) as f64 / 1_000.0),
            category: categories[category_index].to_owned(),
            origin: Some(format!("node-{:02}", pair)),
            target: Some(format!("node-{:02}", (pair * 7 + phase) % 13)),
            magnitude,
            direction,
            labels,
        })
    }
}

struct SplitMix64(u64);

impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut value = self.0;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        value ^ (value >> 31)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_seed_is_deterministic_but_other_seeds_diverge() {
        let first: Vec<_> = DemoSource::new(42).take(80).collect();
        let second: Vec<_> = DemoSource::new(42).take(80).collect();
        let other: Vec<_> = DemoSource::new(43).take(80).collect();
        assert_eq!(first, second);
        assert_ne!(first, other);
        assert!(first.iter().any(|event| event.magnitude > 1_000.0));
        assert!(
            first
                .windows(2)
                .any(|pair| pair[0].direction != pair[1].direction)
        );
    }
}
