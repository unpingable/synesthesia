use std::{path::PathBuf, time::Duration};

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::trigger::TriggerSpec;

#[derive(Debug, Parser)]
#[command(
    name = "synesthesia",
    version,
    about = "Data in. Terminal weather out.",
    long_about = "Synesthesia turns live machine activity into a terminal-native visual instrument."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Generate a deterministic, lively synthetic activity stream.
    Demo(DemoArgs),
    /// Read activity from standard input.
    Stdin(StdinArgs),
    /// Replay a normalized NDJSON recording.
    Replay(ReplayArgs),
    /// Render unprivileged Linux process and host activity from /proc.
    Proc(ProcArgs),
    /// Experimental Linux kernel activity sources.
    Ebpf(EbpfArgs),
    /// Print the supported NDJSON wire schema and an example.
    Schema,
}

#[derive(Debug, Args)]
pub struct EbpfArgs {
    #[command(subcommand)]
    pub source: EbpfSource,
}

#[derive(Debug, Subcommand)]
pub enum EbpfSource {
    /// Render live Linux scheduler tracepoints.
    Scheduler(SchedulerArgs),
    /// Render live TCP retransmits and resets.
    Tcp(TcpArgs),
}

#[derive(Debug, Args)]
pub struct SchedulerArgs {
    /// Save normalized scheduler events as NDJSON.
    #[arg(long, conflicts_with = "flight_recorder")]
    pub record: Option<PathBuf>,
    #[command(flatten)]
    pub flight: FlightArgs,
    #[command(flatten)]
    pub visual: VisualArgs,
}

#[derive(Debug, Args)]
pub struct TcpArgs {
    /// Save normalized TCP pathology events as NDJSON.
    #[arg(long, conflicts_with = "flight_recorder")]
    pub record: Option<PathBuf>,
    #[command(flatten)]
    pub flight: FlightArgs,
    #[command(flatten)]
    pub visual: VisualArgs,
}

#[derive(Debug, Args)]
pub struct ProcArgs {
    /// Sample only one process ID.
    #[arg(long)]
    pub pid: Option<u32>,
    /// Sampling interval from 50ms through 5s.
    #[arg(long, default_value = "250ms", value_parser = parse_proc_interval)]
    pub interval: Duration,
    /// Replace process names and PIDs with stable session-local identities.
    #[arg(long)]
    pub anonymize: bool,
    /// Save normalized process events as NDJSON.
    #[arg(long)]
    pub record: Option<PathBuf>,
    #[command(flatten)]
    pub visual: VisualArgs,
}

#[derive(Clone, Debug, Args)]
pub struct FlightArgs {
    /// Retain bounded history and publish one triggered incident recording.
    #[arg(long)]
    pub flight_recorder: Option<PathBuf>,
    /// Rolling history retained before the trigger (maximum 30s).
    #[arg(long, default_value = "10s", value_parser = parse_duration)]
    pub pre_trigger: Duration,
    /// Tail captured after the trigger (maximum 30s).
    #[arg(long, default_value = "5s", value_parser = parse_duration)]
    pub post_trigger: Duration,
    /// Typed source-specific semantic trigger (auto or manual by default).
    #[arg(long, default_value = "auto")]
    pub trigger: TriggerSpec,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum DisplayMode {
    #[default]
    Ansi,
    Ascii,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum ViewKind {
    #[default]
    Weather,
    Waterfall,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum Theme {
    #[default]
    Phosphor,
    Amber,
    Cold,
    Monochrome,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum ParticleMode {
    #[default]
    On,
    Off,
}

#[derive(Clone, Debug, Args)]
pub struct VisualArgs {
    /// Rendering vocabulary.
    #[arg(long, value_enum, default_value_t)]
    pub mode: DisplayMode,
    /// Temporal view.
    #[arg(long, value_enum, default_value_t)]
    pub view: ViewKind,
    /// Coherent color palette.
    #[arg(long, value_enum, default_value_t)]
    pub theme: Theme,
    /// Event-driven ember overlay.
    #[arg(long, value_enum, default_value_t)]
    pub particles: ParticleMode,
    /// Render one plain frame and exit.
    #[arg(long)]
    pub snapshot: bool,
    /// Snapshot width (ignored interactively).
    #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u16).range(1..=500))]
    pub width: u16,
    /// Snapshot height (ignored interactively).
    #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u16).range(1..=200))]
    pub height: u16,
}

#[derive(Debug, Args)]
pub struct DemoArgs {
    /// Deterministic stream seed.
    #[arg(long, default_value_t = 7)]
    pub seed: u64,
    #[command(flatten)]
    pub visual: VisualArgs,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum InputFormat {
    Lines,
    Ndjson,
    TsharkTsv,
}

#[derive(Debug, Args)]
pub struct StdinArgs {
    #[arg(long, value_enum)]
    pub format: InputFormat,
    /// Save normalized events as NDJSON.
    #[arg(long)]
    pub record: Option<PathBuf>,
    #[command(flatten)]
    pub visual: VisualArgs,
}

#[derive(Debug, Args)]
pub struct ReplayArgs {
    pub path: PathBuf,
    /// Playback speed multiplier.
    #[arg(long, default_value_t = 1.0, value_parser = positive_f64)]
    pub speed: f64,
    #[command(flatten)]
    pub visual: VisualArgs,
}

fn positive_f64(value: &str) -> Result<f64, String> {
    let parsed: f64 = value.parse().map_err(|_| "expected a number".to_owned())?;
    if parsed.is_finite() && parsed > 0.0 {
        Ok(parsed)
    } else {
        Err("speed must be finite and greater than zero".to_owned())
    }
}

fn parse_duration(value: &str) -> Result<Duration, String> {
    let (number, multiplier) = if let Some(number) = value.strip_suffix("ms") {
        (number, 0.001)
    } else if let Some(number) = value.strip_suffix('s') {
        (number, 1.0)
    } else {
        return Err("duration must end in ms or s (for example 250ms or 10s)".to_owned());
    };
    let seconds: f64 = number
        .parse()
        .map_err(|_| "duration must contain a number".to_owned())?;
    let seconds = seconds * multiplier;
    if seconds.is_finite() && seconds >= 0.0 {
        Ok(Duration::from_secs_f64(seconds))
    } else {
        Err("duration must be finite and non-negative".to_owned())
    }
}

fn parse_proc_interval(value: &str) -> Result<Duration, String> {
    let duration = parse_duration(value)?;
    if (Duration::from_millis(50)..=Duration::from_secs(5)).contains(&duration) {
        Ok(duration)
    } else {
        Err("proc interval must be between 50ms and 5s".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_flight_options_parse_as_typed_bounded_values() {
        let cli = Cli::try_parse_from([
            "synesthesia",
            "ebpf",
            "tcp",
            "--flight-recorder",
            "incident.ndjson",
            "--pre-trigger",
            "2500ms",
            "--post-trigger",
            "5s",
            "--trigger",
            "tcp-retransmit-rate=120",
        ])
        .unwrap();
        let Command::Ebpf(EbpfArgs {
            source: EbpfSource::Tcp(args),
        }) = cli.command
        else {
            panic!("expected TCP command");
        };
        assert_eq!(
            args.flight.flight_recorder,
            Some(PathBuf::from("incident.ndjson"))
        );
        assert_eq!(args.flight.pre_trigger, Duration::from_millis(2_500));
        assert_eq!(args.flight.post_trigger, Duration::from_secs(5));
        assert_eq!(args.flight.trigger, TriggerSpec::TcpRetransmitRate(120.0));
    }

    #[test]
    fn ordinary_record_and_flight_recording_conflict() {
        assert!(
            Cli::try_parse_from([
                "synesthesia",
                "ebpf",
                "scheduler",
                "--record",
                "ordinary.ndjson",
                "--flight-recorder",
                "incident.ndjson",
            ])
            .is_err()
        );
    }

    #[test]
    fn proc_options_are_narrow_and_interval_is_bounded() {
        let cli = Cli::try_parse_from([
            "synesthesia",
            "proc",
            "--pid",
            "42",
            "--interval",
            "500ms",
            "--anonymize",
            "--snapshot",
        ])
        .unwrap();
        let Command::Proc(args) = cli.command else {
            panic!("expected proc command");
        };
        assert_eq!(args.pid, Some(42));
        assert_eq!(args.interval, Duration::from_millis(500));
        assert!(args.anonymize);
        assert!(Cli::try_parse_from(["synesthesia", "proc", "--interval", "10ms"]).is_err());
        assert!(Cli::try_parse_from(["synesthesia", "proc", "--interval", "6s"]).is_err());
    }
}
