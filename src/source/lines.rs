use std::{
    collections::BTreeMap,
    io::BufRead,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Result;

use crate::{
    event::{Direction, NormalizedEvent, SCHEMA_VERSION},
    source::{EventSource, SourceStats, stable_hash},
};

pub struct LineSource<R> {
    reader: R,
    buffer: Vec<u8>,
    stats: SourceStats,
}

impl<R: BufRead> LineSource<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            buffer: Vec::with_capacity(512),
            stats: SourceStats::default(),
        }
    }
}

impl<R: BufRead> EventSource for LineSource<R> {
    fn next_event(&mut self) -> Result<Option<NormalizedEvent>> {
        self.buffer.clear();
        let count = self.reader.read_until(b'\n', &mut self.buffer)?;
        if count == 0 {
            return Ok(None);
        }
        while self
            .buffer
            .last()
            .is_some_and(|byte| matches!(byte, b'\n' | b'\r'))
        {
            self.buffer.pop();
        }
        let hash = stable_hash(&self.buffer);
        let category = ["whisper", "pulse", "spark", "drift"][(hash as usize) % 4];
        let direction = match (hash >> 8) % 4 {
            0 => Direction::Inbound,
            1 => Direction::Outbound,
            2 => Direction::Neutral,
            _ => Direction::Unknown,
        };
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_secs_f64());
        let mut labels = BTreeMap::new();
        labels.insert("lane".to_owned(), ((hash >> 16) % 32).to_string());
        labels.insert(
            "preview".to_owned(),
            String::from_utf8_lossy(&self.buffer)
                .chars()
                .filter(|character| !character.is_control())
                .take(48)
                .collect(),
        );
        self.stats.accepted += 1;
        Ok(Some(NormalizedEvent {
            v: SCHEMA_VERSION,
            timestamp,
            category: category.to_owned(),
            origin: Some(format!("line:{:04x}", hash & 0xffff)),
            target: None,
            magnitude: count.max(1) as f64,
            direction,
            labels,
        }))
    }

    fn stats(&self) -> SourceStats {
        self.stats
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    #[test]
    fn line_hashing_is_stable_and_content_sensitive() {
        let mut first = LineSource::new(Cursor::new(b"alpha\nalpha\nbeta\n"));
        let a = first.next_event().unwrap().unwrap();
        let a_again = first.next_event().unwrap().unwrap();
        let b = first.next_event().unwrap().unwrap();
        assert_eq!(a.origin, a_again.origin);
        assert_eq!(a.category, a_again.category);
        assert_ne!(a.origin, b.origin);
    }

    #[test]
    fn binaryish_lines_are_lossy_not_fatal() {
        let mut source = LineSource::new(Cursor::new(&b"a\xff\0b\n"[..]));
        let event = source.next_event().unwrap().unwrap();
        assert_eq!(event.magnitude, 5.0);
        assert_eq!(source.stats().malformed, 0);
    }
}
