use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

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
    #[arg(long)]
    pub record: Option<PathBuf>,
    #[command(flatten)]
    pub visual: VisualArgs,
}

#[derive(Debug, Args)]
pub struct TcpArgs {
    /// Save normalized TCP pathology events as NDJSON.
    #[arg(long)]
    pub record: Option<PathBuf>,
    #[command(flatten)]
    pub visual: VisualArgs,
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
