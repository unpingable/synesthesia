#![forbid(unsafe_code)]

use std::io::{self, BufReader};

use anyhow::{Result, bail};
use clap::Parser;
use synesthesia::{
    cli::{Cli, Command, InputFormat},
    event,
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
            while let Some(event) = source.next_event()? {
                println!("{}", serde_json::to_string(&event)?);
            }
            if source.stats().malformed > 0 {
                eprintln!("malformed records: {}", source.stats().malformed);
            }
        }
        Command::Replay(_) => bail!("replay arrives in Stage 2"),
    }
    Ok(())
}
