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
    }
    Ok(())
}
