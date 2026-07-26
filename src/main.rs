#![forbid(unsafe_code)]

use std::{
    io::{self, BufReader},
    thread,
};

use anyhow::{Result, bail};
use clap::Parser;
use synesthesia::{
    cli::{Cli, Command, InputFormat},
    event,
    recording::{Recorder, ReplaySource},
    source::{EventSource, demo::DemoSource, lines::LineSource, ndjson::NdjsonSource},
};

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Schema => {
            println!("{}", event::schema_document()?);
        }
        Command::Demo(args) => {
            for event in DemoSource::new(args.seed).take(200) {
                println!("{}", serde_json::to_string(&event)?);
            }
        }
        Command::Stdin(args) => {
            let stdin = io::stdin();
            let mut source: Box<dyn EventSource> = match args.format {
                InputFormat::Lines => Box::new(LineSource::new(BufReader::new(stdin.lock()))),
                InputFormat::Ndjson => Box::new(NdjsonSource::new(BufReader::new(stdin.lock()))),
                InputFormat::TsharkTsv => bail!("tshark-tsv arrives in Stage 4"),
            };
            let mut recorder = args.record.as_deref().map(Recorder::create).transpose()?;
            while let Some(event) = source.next_event()? {
                if let Some(recorder) = &mut recorder {
                    recorder.record(&event)?;
                }
                println!("{}", serde_json::to_string(&event)?);
            }
            if let Some(recorder) = recorder {
                recorder.finish()?;
            }
            if source.stats().malformed > 0 {
                eprintln!("malformed records: {}", source.stats().malformed);
            }
        }
        Command::Replay(args) => {
            let mut replay = ReplaySource::open(&args.path, args.speed)?;
            while let Some((delay, event)) = replay.next_timed()? {
                thread::sleep(delay);
                println!("{}", serde_json::to_string(&event)?);
            }
            if replay.stats().malformed > 0 {
                eprintln!("malformed records: {}", replay.stats().malformed);
            }
        }
    }
    Ok(())
}
