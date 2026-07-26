#![forbid(unsafe_code)]

use anyhow::Result;
use clap::Parser;
use synesthesia::{
    app,
    cli::{Cli, Command},
    event,
};

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Schema => {
            println!("{}", event::schema_document()?);
        }
        Command::Demo(args) => app::run_demo(args)?,
        Command::Stdin(args) => app::run_stdin(args)?,
        Command::Replay(args) => app::run_replay(args)?,
        Command::Ebpf(args) => match args.source {
            synesthesia::cli::EbpfSource::Scheduler(args) => app::run_scheduler(args)?,
            synesthesia::cli::EbpfSource::Tcp(args) => app::run_tcp(args)?,
        },
    }
    Ok(())
}
