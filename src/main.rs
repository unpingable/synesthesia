#![forbid(unsafe_code)]

use std::io::Write;

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
        Command::Doctor(args) => {
            let outcome = match synesthesia::doctor::run(&args) {
                Ok(outcome) => outcome,
                Err(error) => {
                    eprintln!("doctor could not construct a valid report: {error}");
                    std::process::exit(2);
                }
            };
            if let Err(error) = std::io::stdout().write_all(outcome.output.as_bytes()) {
                eprintln!("doctor could not write its report: {error}");
                std::process::exit(2);
            }
            if outcome.exit_code != 0 {
                std::process::exit(outcome.exit_code.into());
            }
        }
        Command::Completions(args) => {
            synesthesia::generated::completions(args.shell, &mut std::io::stdout());
        }
        Command::Manpage => {
            synesthesia::generated::manpage(&mut std::io::stdout())?;
        }
        Command::Demo(args) => app::run_demo(args)?,
        Command::Stdin(args) => app::run_stdin(args)?,
        Command::Replay(args) => app::run_replay(args)?,
        Command::Proc(args) => app::run_proc(args)?,
        Command::Ebpf(args) => match args.source {
            synesthesia::cli::EbpfSource::Scheduler(args) => app::run_scheduler(args)?,
            synesthesia::cli::EbpfSource::Tcp(args) => app::run_tcp(args)?,
        },
    }
    Ok(())
}
