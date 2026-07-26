use std::{
    collections::BTreeMap,
    io::BufRead,
    net::{IpAddr, Ipv4Addr},
};

use anyhow::Result;

use crate::{
    event::{Direction, NormalizedEvent, SCHEMA_VERSION},
    source::{EventSource, SourceStats},
};

pub const COLUMN_COUNT: usize = 11;

pub struct TsharkTsvSource<R> {
    reader: R,
    buffer: Vec<u8>,
    stats: SourceStats,
}

impl<R: BufRead> TsharkTsvSource<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            buffer: Vec::with_capacity(512),
            stats: SourceStats::default(),
        }
    }

    fn parse_row(row: &[u8]) -> Result<NormalizedEvent, &'static str> {
        let text = std::str::from_utf8(row).map_err(|_| "row is not UTF-8")?;
        let columns: Vec<_> = text.trim_end_matches(['\r', '\n']).split('\t').collect();
        if columns.len() != COLUMN_COUNT {
            return Err("expected exactly 11 tab-separated fields");
        }
        let timestamp = optional_f64(columns[0]).ok_or("invalid timestamp")?;
        let source_ip = first_present(columns[1], columns[2]);
        let target_ip = first_present(columns[3], columns[4]);
        let protocol = present(columns[5]).unwrap_or("unknown");
        let magnitude = present(columns[6])
            .map(str::parse::<f64>)
            .transpose()
            .map_err(|_| "invalid frame length")?
            .unwrap_or(1.0);
        if !magnitude.is_finite() || magnitude < 0.0 {
            return Err("invalid frame length");
        }
        let source_port = first_present(columns[7], columns[8]);
        let target_port = first_present(columns[9], columns[10]);
        let origin = endpoint(source_ip, source_port);
        let target = endpoint(target_ip, target_port);
        let direction = infer_direction(source_ip, target_ip);
        let mut labels = BTreeMap::new();
        labels.insert("protocol".to_owned(), protocol.to_owned());
        if let Some(ip) = source_ip.and_then(|value| value.parse::<IpAddr>().ok()) {
            labels.insert(
                "ip_version".to_owned(),
                if ip.is_ipv4() { "4" } else { "6" }.to_owned(),
            );
        }
        Ok(NormalizedEvent {
            v: SCHEMA_VERSION,
            timestamp,
            category: protocol.to_ascii_lowercase(),
            origin,
            target,
            magnitude,
            direction,
            labels,
        })
    }
}

impl<R: BufRead> EventSource for TsharkTsvSource<R> {
    fn next_event(&mut self) -> Result<Option<NormalizedEvent>> {
        loop {
            self.buffer.clear();
            if self.reader.read_until(b'\n', &mut self.buffer)? == 0 {
                return Ok(None);
            }
            match Self::parse_row(&self.buffer) {
                Ok(event) => {
                    self.stats.accepted += 1;
                    return Ok(Some(event));
                }
                Err(error) => {
                    self.stats.malformed += 1;
                    eprintln!("synesthesia: skipped malformed tshark TSV row: {error}");
                }
            }
        }
    }

    fn stats(&self) -> SourceStats {
        self.stats
    }
}

fn present(value: &str) -> Option<&str> {
    (!value.is_empty()).then_some(value)
}

fn first_present<'a>(first: &'a str, second: &'a str) -> Option<&'a str> {
    present(first).or_else(|| present(second))
}

fn optional_f64(value: &str) -> Option<Option<f64>> {
    if value.is_empty() {
        return Some(None);
    }
    value
        .parse::<f64>()
        .ok()
        .filter(|parsed| parsed.is_finite())
        .map(Some)
}

fn endpoint(address: Option<&str>, port: Option<&str>) -> Option<String> {
    match (address, port) {
        (Some(address), Some(port)) if address.contains(':') => Some(format!("[{address}]:{port}")),
        (Some(address), Some(port)) => Some(format!("{address}:{port}")),
        (Some(address), None) => Some(address.to_owned()),
        (None, Some(port)) => Some(format!("port:{port}")),
        (None, None) => None,
    }
}

fn infer_direction(source: Option<&str>, target: Option<&str>) -> Direction {
    let (Some(source), Some(target)) = (
        source.and_then(|value| value.parse::<IpAddr>().ok()),
        target.and_then(|value| value.parse::<IpAddr>().ok()),
    ) else {
        return Direction::Unknown;
    };
    if source == target {
        return Direction::Neutral;
    }
    match (is_localish(source), is_localish(target)) {
        (true, false) => Direction::Outbound,
        (false, true) => Direction::Inbound,
        _ => Direction::Unknown,
    }
}

fn is_localish(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            address.is_private()
                || address.is_loopback()
                || address.is_link_local()
                || address == Ipv4Addr::UNSPECIFIED
        }
        IpAddr::V6(address) => {
            let first = address.octets()[0];
            address.is_loopback()
                || address.is_unspecified()
                || (first & 0xfe) == 0xfc
                || (address.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    #[test]
    fn exact_fixture_maps_ipv4_ipv6_protocol_ports_and_direction() {
        let fixture = include_bytes!("../../tests/fixtures/tshark-fields.tsv");
        let mut source = TsharkTsvSource::new(Cursor::new(fixture));
        let mut events = Vec::new();
        while let Some(event) = source.next_event().unwrap() {
            events.push(event);
        }
        assert_eq!(events.len(), 5);
        assert_eq!(events[0].origin.as_deref(), Some("10.0.0.4:54321"));
        assert_eq!(events[0].target.as_deref(), Some("93.184.216.34:443"));
        assert_eq!(events[0].direction, Direction::Outbound);
        assert_eq!(events[1].category, "dns");
        assert_eq!(events[1].origin.as_deref(), Some("8.8.8.8:53"));
        assert_eq!(events[1].direction, Direction::Inbound);
        assert_eq!(events[2].origin.as_deref(), Some("[fd00::4]:5353"));
        assert_eq!(events[2].labels["ip_version"], "6");
        assert_eq!(events[4].magnitude, 9_000.0);
        assert_eq!(source.stats().malformed, 0);
    }

    #[test]
    fn malformed_rows_are_counted_and_following_rows_survive() {
        let input = concat!(
            "too\tfew\n",
            "not-a-time\t10.0.0.1\t\t10.0.0.2\t\tTCP\t60\t1\t\t2\t\n",
            "1720000000.0\t10.0.0.1\t\t10.0.0.2\t\tTCP\t60\t1\t\t2\t\n"
        );
        let mut source = TsharkTsvSource::new(Cursor::new(input));
        let event = source.next_event().unwrap().unwrap();
        assert_eq!(event.magnitude, 60.0);
        assert_eq!(source.stats().malformed, 2);
    }
}
