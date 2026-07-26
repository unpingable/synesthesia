use std::io::BufRead;

use anyhow::Result;

use crate::{
    event::NormalizedEvent,
    source::{EventSource, SourceStats, read_bounded_record},
};

pub struct NdjsonSource<R> {
    reader: R,
    buffer: Vec<u8>,
    stats: SourceStats,
}

impl<R: BufRead> NdjsonSource<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            buffer: Vec::with_capacity(1024),
            stats: SourceStats::default(),
        }
    }
}

impl<R: BufRead> EventSource for NdjsonSource<R> {
    fn next_event(&mut self) -> Result<Option<NormalizedEvent>> {
        loop {
            let (count, oversized) = read_bounded_record(&mut self.reader, &mut self.buffer)?;
            if count == 0 {
                return Ok(None);
            }
            if oversized {
                self.stats.malformed += 1;
                eprintln!("synesthesia: skipped NDJSON record larger than 64 KiB");
                continue;
            }
            if self.buffer.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            let parsed = serde_json::from_slice::<NormalizedEvent>(&self.buffer)
                .map_err(anyhow::Error::from)
                .and_then(|event| event.validate().map_err(anyhow::Error::from));
            match parsed {
                Ok(event) => {
                    self.stats.accepted += 1;
                    return Ok(Some(event));
                }
                Err(error) => {
                    self.stats.malformed += 1;
                    eprintln!("synesthesia: skipped malformed NDJSON record: {error}");
                }
            }
        }
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
    fn malformed_records_increment_errors_and_stream_continues() {
        let input = concat!(
            "not json\n",
            "{\"v\":9,\"category\":\"future\",\"magnitude\":1}\n",
            "{\"v\":1,\"category\":\"alive\",\"magnitude\":3}\n"
        );
        let mut source = NdjsonSource::new(Cursor::new(input));
        let event = source.next_event().unwrap().unwrap();
        assert_eq!(event.category, "alive");
        assert_eq!(source.stats().accepted, 1);
        assert_eq!(source.stats().malformed, 2);
        assert!(source.next_event().unwrap().is_none());
    }

    #[test]
    fn oversized_record_is_discarded_without_losing_the_next_record() {
        let huge = format!(
            "{{\"v\":1,\"category\":\"{}\",\"magnitude\":1}}\n\
             {{\"v\":1,\"category\":\"survivor\",\"magnitude\":2}}\n",
            "x".repeat(70_000)
        );
        let mut source = NdjsonSource::new(Cursor::new(huge));
        let event = source.next_event().unwrap().unwrap();
        assert_eq!(event.category, "survivor");
        assert_eq!(source.stats().malformed, 1);
    }
}
