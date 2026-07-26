use std::{
    fs::File,
    io::{BufReader, BufWriter, Write},
    path::Path,
    time::Duration,
};

use anyhow::{Context, Result};

use crate::{
    event::NormalizedEvent,
    flight_recorder::{FlightSource, METADATA_CATEGORY, parse_metadata_event},
    source::{EventSource, SourceStats, ndjson::NdjsonSource},
};

pub const UNTIMED_FALLBACK: Duration = Duration::from_millis(50);

pub struct Recorder {
    writer: BufWriter<File>,
}

impl Recorder {
    pub fn create(path: &Path) -> Result<Self> {
        let file = File::create(path)
            .with_context(|| format!("could not create recording {}", path.display()))?;
        Ok(Self {
            writer: BufWriter::new(file),
        })
    }

    pub fn record(&mut self, event: &NormalizedEvent) -> Result<()> {
        serde_json::to_writer(&mut self.writer, event)?;
        self.writer.write_all(b"\n")?;
        Ok(())
    }

    pub fn finish(mut self) -> Result<()> {
        self.writer.flush()?;
        Ok(())
    }
}

pub struct ReplaySource {
    source: NdjsonSource<BufReader<File>>,
    previous_timestamp: Option<f64>,
    speed: f64,
    flight_source: Option<FlightSource>,
}

impl ReplaySource {
    pub fn open(path: &Path, speed: f64) -> Result<Self> {
        let file = File::open(path)
            .with_context(|| format!("could not open replay {}", path.display()))?;
        Ok(Self {
            source: NdjsonSource::new(BufReader::new(file)),
            previous_timestamp: None,
            speed,
            flight_source: None,
        })
    }

    pub fn next_timed(&mut self) -> Result<Option<(Duration, NormalizedEvent)>> {
        let Some(mut event) = self.source.next_event()? else {
            return Ok(None);
        };
        if event.category == METADATA_CATEGORY {
            let metadata = parse_metadata_event(&event)?;
            self.flight_source = metadata.map(|metadata| metadata.source);
            return Ok(Some((Duration::ZERO, event)));
        }
        if event.category == crate::flight_recorder::TRIGGER_CATEGORY {
            if let Some(source) = self.flight_source {
                event
                    .labels
                    .entry("source".to_owned())
                    .or_insert_with(|| source.as_str().to_owned());
            }
        }
        let delay = replay_delay(self.previous_timestamp, event.timestamp, self.speed);
        if let Some(timestamp) = event.timestamp {
            self.previous_timestamp = Some(timestamp);
        }
        Ok(Some((delay, event)))
    }

    pub fn stats(&self) -> SourceStats {
        self.source.stats()
    }
}

pub fn replay_delay(previous: Option<f64>, current: Option<f64>, speed: f64) -> Duration {
    match (previous, current) {
        (Some(previous), Some(current)) if current >= previous => {
            Duration::from_secs_f64(((current - previous) / speed).min(60.0))
        }
        _ => UNTIMED_FALLBACK.div_f64(speed),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use crate::source::demo::DemoSource;

    use super::*;

    fn temporary_path(label: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("synesthesia-{label}-{unique}.ndjson"))
    }

    #[test]
    fn record_replay_round_trip_preserves_events() {
        let path = temporary_path("roundtrip");
        let expected: Vec<_> = DemoSource::new(9).take(12).collect();
        let mut recorder = Recorder::create(&path).unwrap();
        for event in &expected {
            recorder.record(event).unwrap();
        }
        recorder.finish().unwrap();

        let mut replay = ReplaySource::open(&path, 1.0).unwrap();
        let mut actual = Vec::new();
        while let Some((_, event)) = replay.next_timed().unwrap() {
            actual.push(event);
        }
        fs::remove_file(path).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn replay_timing_respects_speed_and_fallback() {
        assert_eq!(
            replay_delay(Some(10.0), Some(10.5), 2.0),
            Duration::from_millis(250)
        );
        assert_eq!(
            replay_delay(Some(10.0), Some(10.5), 0.5),
            Duration::from_secs(1)
        );
        assert_eq!(replay_delay(None, None, 2.0), Duration::from_millis(25));
    }

    #[test]
    fn sanitized_scheduler_fixture_replays_switches_wakeups_and_migrations() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/scheduler.ndjson");
        let mut replay = ReplaySource::open(&path, 2.0).unwrap();
        let mut categories = std::collections::BTreeSet::new();
        let mut total_delay = Duration::ZERO;
        while let Some((delay, event)) = replay.next_timed().unwrap() {
            total_delay += delay;
            categories.insert(event.category);
        }
        assert_eq!(
            categories,
            std::collections::BTreeSet::from([
                "sched.migrate".to_owned(),
                "sched.switch".to_owned(),
                "sched.wakeup".to_owned(),
            ])
        );
        assert_eq!(total_delay, Duration::from_micros(2_041_500));
    }

    #[test]
    fn sanitized_tcp_fixture_replays_pathology_without_privilege() {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tcp-pathology.ndjson");
        let mut replay = ReplaySource::open(&path, 2.0).unwrap();
        let mut categories = std::collections::BTreeSet::new();
        let mut total_delay = Duration::ZERO;
        while let Some((delay, event)) = replay.next_timed().unwrap() {
            total_delay += delay;
            categories.insert(event.category);
        }
        assert_eq!(
            categories,
            std::collections::BTreeSet::from([
                "tcp.reset.receive".to_owned(),
                "tcp.reset.send".to_owned(),
                "tcp.retransmit".to_owned(),
            ])
        );
        assert_eq!(total_delay, Duration::from_millis(3_525));
    }

    #[test]
    fn flight_metadata_is_validated_skipped_and_trigger_is_exposed() {
        for (fixture, source) in [
            ("tcp-flight-incident.ndjson", "tcp"),
            ("scheduler-flight-incident.ndjson", "scheduler"),
        ] {
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures")
                .join(fixture);
            let mut replay = ReplaySource::open(&path, 1.0).unwrap();
            let mut categories = Vec::new();
            let mut trigger = None;
            while let Some((_, event)) = replay.next_timed().unwrap() {
                if event.category == crate::flight_recorder::TRIGGER_CATEGORY {
                    trigger = Some(event.labels.clone());
                }
                categories.push(event.category);
            }
            let trigger = trigger.expect("fixture trigger marker");
            assert_eq!(trigger["source"], source);
            assert_eq!(trigger[crate::flight_recorder::PHASE_LABEL], "trigger");
            assert_eq!(
                categories
                    .iter()
                    .filter(|category| category.as_str() == crate::flight_recorder::TRIGGER_CATEGORY)
                    .count(),
                1
            );
            assert_eq!(
                categories
                    .iter()
                    .filter(|category| category.as_str() == METADATA_CATEGORY)
                    .count(),
                2
            );
        }
    }

    #[test]
    fn replay_refuses_unsupported_flight_metadata() {
        let path = temporary_path("flight-version");
        fs::write(
            &path,
            "{\"v\":1,\"category\":\"synesthesia.flight.metadata\",\"magnitude\":0,\"direction\":\"neutral\",\"labels\":{\"format_version\":\"99\",\"record\":\"start\",\"source\":\"tcp\"}}\n",
        )
        .unwrap();
        let mut replay = ReplaySource::open(&path, 1.0).unwrap();
        let error = replay.next_timed().unwrap_err().to_string();
        assert!(error.contains("unsupported flight metadata version 99"));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn replay_refuses_truncated_flight_metadata_without_root() {
        let path = temporary_path("flight-malformed");
        fs::write(
            &path,
            "{\"v\":1,\"category\":\"synesthesia.flight.metadata\",\"magnitude\":0,\"direction\":\"neutral\",\"labels\":{\"format_version\":\"1\",\"record\":\"start\",\"source\":\"tcp\"}}\n",
        )
        .unwrap();
        let mut replay = ReplaySource::open(&path, 1.0).unwrap();
        let error = replay.next_timed().unwrap_err().to_string();
        assert!(error.contains("missing configured_pre_trigger_seconds"));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn orderly_partial_incident_remains_stream_recoverable() {
        use crate::flight_recorder::{
            FlightConfig, FlightRecorder, FlightSource, HostMetadata, IncidentLosses, TriggerInfo,
            part_path,
        };

        let path = temporary_path("flight-partial");
        let mut config = FlightConfig::new(path.clone(), FlightSource::Tcp);
        config.pre_trigger = Duration::from_secs(1);
        config.post_trigger = Duration::from_secs(5);
        config.host = HostMetadata {
            kernel: "test".to_owned(),
            architecture: "test".to_owned(),
        };
        let mut flight = FlightRecorder::new(config).unwrap();
        flight.arm(0.0).unwrap();
        let event = DemoSource::new(9).next().unwrap();
        flight
            .observe(event, 0.1, IncidentLosses::default())
            .unwrap();
        flight
            .trigger(
                0.2,
                Some(0.2),
                TriggerInfo::manual(),
                IncidentLosses::default(),
            )
            .unwrap();
        drop(flight);

        let partial = part_path(&path);
        let mut replay = ReplaySource::open(&partial, 1.0).unwrap();
        let mut categories = Vec::new();
        while let Some((_, event)) = replay.next_timed().unwrap() {
            categories.push(event.category);
        }
        assert!(categories.contains(&crate::flight_recorder::TRIGGER_CATEGORY.to_owned()));
        assert!(!path.exists());
        fs::remove_file(partial).unwrap();
    }
}
