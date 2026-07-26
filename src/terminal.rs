use std::{
    io::{self, Stdout},
    panic,
};

use anyhow::Result;
use crossterm::{
    cursor::{Hide, Show},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};

pub type LiveTerminal = Terminal<CrosstermBackend<Stdout>>;
type PanicHook = Box<dyn Fn(&panic::PanicHookInfo<'_>) + Sync + Send + 'static>;

pub struct TerminalSession {
    terminal: LiveTerminal,
    previous_hook: Option<PanicHook>,
}

impl TerminalSession {
    pub fn enter() -> Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        if let Err(error) = execute!(stdout, EnterAlternateScreen, Hide) {
            let _ = disable_raw_mode();
            return Err(error.into());
        }
        let previous_hook = panic::take_hook();
        panic::set_hook(Box::new(|info| {
            restore();
            eprintln!("synesthesia panicked: {info}");
        }));
        let terminal = Terminal::new(CrosstermBackend::new(stdout))?;
        Ok(Self {
            terminal,
            previous_hook: Some(previous_hook),
        })
    }

    pub fn terminal_mut(&mut self) -> &mut LiveTerminal {
        &mut self.terminal
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        restore();
        if let Some(previous_hook) = self.previous_hook.take() {
            let _ = panic::take_hook();
            panic::set_hook(previous_hook);
        }
    }
}

fn restore() {
    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), Show, LeaveAlternateScreen);
}
