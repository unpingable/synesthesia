use std::{
    io::{self, BufReader, IsTerminal},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Result, bail};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};

use crate::{
    cli::{DemoArgs, DisplayMode, InputFormat, ReplayArgs, StdinArgs, Theme, ViewKind, VisualArgs},
    ingestion::{Ingress, event_buffer},
    model::TemporalModel,
    recording::{Recorder, ReplaySource},
    render::GridWidget,
    source::{EventSource, demo::DemoSource, lines::LineSource, ndjson::NdjsonSource},
    terminal::TerminalSession,
    view::{ViewOptions, compose},
};

const CHANNEL_CAPACITY: usize = 2_048;
const MAX_DRAIN_PER_FRAME: usize = 1_024;
const FRAME_TIME: Duration = Duration::from_millis(33);

enum Producer {
    Demo {
        seed: u64,
    },
    Stdin {
        format: InputFormat,
        record: Option<std::path::PathBuf>,
    },
    Replay {
        args: ReplayArgs,
    },
}

#[derive(Default)]
struct ProducerStats {
    malformed: AtomicU64,
}

pub fn run_demo(args: DemoArgs) -> Result<()> {
    if args.visual.snapshot || !io::stdout().is_terminal() {
        return snapshot_demo(&args);
    }
    run_interactive(Producer::Demo { seed: args.seed }, args.visual)
}

pub fn run_stdin(args: StdinArgs) -> Result<()> {
    if args.format == InputFormat::TsharkTsv {
        bail!("tshark-tsv arrives in Stage 4");
    }
    if args.visual.snapshot || !io::stdout().is_terminal() {
        return snapshot_stdin(args);
    }
    let visual = args.visual.clone();
    run_interactive(
        Producer::Stdin {
            format: args.format,
            record: args.record,
        },
        visual,
    )
}

pub fn run_replay(args: ReplayArgs) -> Result<()> {
    if args.visual.snapshot || !io::stdout().is_terminal() {
        return snapshot_replay(args);
    }
    let visual = args.visual.clone();
    run_interactive(Producer::Replay { args }, visual)
}

fn snapshot_demo(args: &DemoArgs) -> Result<()> {
    let mut model = TemporalModel::default();
    for (index, event) in DemoSource::new(args.seed).take(260).enumerate() {
        model.ingest(event, index as f64 * 0.035);
    }
    print_snapshot(&model, &args.visual, 0)
}

fn snapshot_stdin(args: StdinArgs) -> Result<()> {
    let stdin = io::stdin();
    let mut source: Box<dyn EventSource> = match args.format {
        InputFormat::Lines => Box::new(LineSource::new(BufReader::new(stdin.lock()))),
        InputFormat::Ndjson => Box::new(NdjsonSource::new(BufReader::new(stdin.lock()))),
        InputFormat::TsharkTsv => bail!("tshark-tsv arrives in Stage 4"),
    };
    let mut recorder = args.record.as_deref().map(Recorder::create).transpose()?;
    let mut model = TemporalModel::default();
    let mut index = 0_u64;
    while let Some(event) = source.next_event()? {
        if let Some(recorder) = &mut recorder {
            recorder.record(&event)?;
        }
        model.ingest(event, index as f64 * 0.035);
        index += 1;
    }
    if let Some(recorder) = recorder {
        recorder.finish()?;
    }
    print_snapshot(&model, &args.visual, source.stats().malformed)
}

fn snapshot_replay(args: ReplayArgs) -> Result<()> {
    let mut replay = ReplaySource::open(&args.path, args.speed)?;
    let mut model = TemporalModel::default();
    let mut now = 0.0;
    while let Some((delay, event)) = replay.next_timed()? {
        now += delay.as_secs_f64();
        model.ingest(event, now);
    }
    print_snapshot(&model, &args.visual, replay.stats().malformed)
}

fn print_snapshot(model: &TemporalModel, visual: &VisualArgs, malformed: u64) -> Result<()> {
    let options = ViewOptions {
        mode: visual.mode,
        view: visual.view,
        gain: 1.0,
        paused: false,
        malformed,
        dropped: 0,
        help: false,
    };
    let frame = compose(&model.snapshot(), visual.width, visual.height, &options);
    println!("{}", frame.plain_text());
    Ok(())
}

fn run_interactive(producer: Producer, visual: VisualArgs) -> Result<()> {
    let (ingress, buffer) = event_buffer(CHANNEL_CAPACITY);
    let producer_stats = Arc::new(ProducerStats::default());
    spawn_producer(producer, ingress, Arc::clone(&producer_stats));

    let mut session = TerminalSession::enter()?;
    let truecolor = terminal_truecolor();
    let ansi_allowed = terminal_color();
    let mut state = UiState {
        mode: if visual.mode == DisplayMode::Ansi && !ansi_allowed {
            DisplayMode::Ascii
        } else {
            visual.mode
        },
        view: visual.view,
        theme: visual.theme,
        gain: 1.0,
        decay: 2.8,
        paused: false,
        help: false,
    };
    let mut model = TemporalModel::new(state.decay);
    let started = Instant::now();
    let mut drained = Vec::with_capacity(MAX_DRAIN_PER_FRAME);

    loop {
        let frame_started = Instant::now();
        let now = started.elapsed().as_secs_f64();
        if !state.paused {
            drained.clear();
            buffer.drain_into(&mut drained, MAX_DRAIN_PER_FRAME);
            for incoming in drained.drain(..) {
                model.ingest(incoming, now);
            }
            model.advance(now);
        }
        let size = session.terminal_mut().size()?;
        let options = ViewOptions {
            mode: state.mode,
            view: state.view,
            gain: state.gain,
            paused: state.paused,
            malformed: producer_stats.malformed.load(Ordering::Relaxed),
            dropped: buffer.dropped(),
            help: state.help,
        };
        let frame = compose(&model.snapshot(), size.width, size.height, &options);
        session.terminal_mut().draw(|terminal_frame| {
            terminal_frame.render_widget(
                GridWidget {
                    frame: &frame,
                    mode: state.mode,
                    theme: state.theme,
                    truecolor,
                },
                terminal_frame.area(),
            );
        })?;

        let remaining = FRAME_TIME.saturating_sub(frame_started.elapsed());
        if event::poll(remaining)? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if handle_key(key.code, key.modifiers, &mut state, ansi_allowed) {
                        break;
                    }
                    model.set_decay(state.decay);
                }
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
    }
    Ok(())
}

struct UiState {
    mode: DisplayMode,
    view: ViewKind,
    theme: Theme,
    gain: f32,
    decay: f64,
    paused: bool,
    help: bool,
}

fn handle_key(
    code: KeyCode,
    modifiers: KeyModifiers,
    state: &mut UiState,
    ansi_allowed: bool,
) -> bool {
    if code == KeyCode::Esc
        || code == KeyCode::Char('q')
        || (code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL))
    {
        return true;
    }
    match code {
        KeyCode::Char(' ') => state.paused = !state.paused,
        KeyCode::Char('1') => state.view = ViewKind::Weather,
        KeyCode::Char('2') => state.view = ViewKind::Waterfall,
        KeyCode::Char('a') if ansi_allowed => {
            state.mode = match state.mode {
                DisplayMode::Ansi => DisplayMode::Ascii,
                DisplayMode::Ascii => DisplayMode::Ansi,
            };
        }
        KeyCode::Char('c') => {
            state.theme = match state.theme {
                Theme::Phosphor => Theme::Amber,
                Theme::Amber => Theme::Cold,
                Theme::Cold => Theme::Monochrome,
                Theme::Monochrome => Theme::Phosphor,
            };
        }
        KeyCode::Char('+') | KeyCode::Char('=') => state.gain = (state.gain * 1.15).min(4.0),
        KeyCode::Char('-') => state.gain = (state.gain / 1.15).max(0.2),
        KeyCode::Char('[') => state.decay = (state.decay / 1.15).max(0.2),
        KeyCode::Char(']') => state.decay = (state.decay * 1.15).min(30.0),
        KeyCode::Char('h' | '?') => state.help = !state.help,
        _ => {}
    }
    false
}

fn spawn_producer(producer: Producer, ingress: Ingress, stats: Arc<ProducerStats>) {
    thread::spawn(move || match producer {
        Producer::Demo { seed } => {
            for generated in DemoSource::new(seed) {
                if !ingress.submit(generated) {
                    thread::yield_now();
                }
                thread::sleep(Duration::from_millis(24));
            }
        }
        Producer::Stdin { format, record } => {
            let stdin = io::stdin();
            let mut source: Box<dyn EventSource> = match format {
                InputFormat::Lines => Box::new(LineSource::new(BufReader::new(stdin.lock()))),
                InputFormat::Ndjson => Box::new(NdjsonSource::new(BufReader::new(stdin.lock()))),
                InputFormat::TsharkTsv => return,
            };
            let mut recorder = record
                .as_deref()
                .map(Recorder::create)
                .transpose()
                .ok()
                .flatten();
            loop {
                match source.next_event() {
                    Ok(Some(incoming)) => {
                        if let Some(recorder) = &mut recorder {
                            let _ = recorder.record(&incoming);
                        }
                        ingress.submit(incoming);
                    }
                    Ok(None) | Err(_) => break,
                }
                stats
                    .malformed
                    .store(source.stats().malformed, Ordering::Relaxed);
            }
            stats
                .malformed
                .store(source.stats().malformed, Ordering::Relaxed);
            if let Some(recorder) = recorder {
                let _ = recorder.finish();
            }
        }
        Producer::Replay { args } => {
            let Ok(mut replay) = ReplaySource::open(&args.path, args.speed) else {
                return;
            };
            while let Ok(Some((delay, incoming))) = replay.next_timed() {
                thread::sleep(delay);
                ingress.submit(incoming);
                stats
                    .malformed
                    .store(replay.stats().malformed, Ordering::Relaxed);
            }
        }
    });
}

fn terminal_color() -> bool {
    std::env::var_os("NO_COLOR").is_none()
        && std::env::var("TERM").map_or(true, |term| term != "dumb")
}

fn terminal_truecolor() -> bool {
    terminal_color()
        && std::env::var("COLORTERM")
            .is_ok_and(|value| value.contains("truecolor") || value.contains("24bit"))
}

#[cfg(test)]
mod tests {
    use ratatui::{Terminal, backend::TestBackend};

    use super::*;

    #[test]
    fn ratatui_test_backend_renders_the_composed_grid() {
        let mut model = TemporalModel::default();
        for (index, generated) in DemoSource::new(4).take(80).enumerate() {
            model.ingest(generated, index as f64 * 0.03);
        }
        let options = ViewOptions {
            mode: DisplayMode::Ascii,
            view: ViewKind::Weather,
            gain: 1.0,
            paused: false,
            malformed: 0,
            dropped: 0,
            help: false,
        };
        let composed = compose(&model.snapshot(), 60, 18, &options);
        let backend = TestBackend::new(60, 18);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                frame.render_widget(
                    GridWidget {
                        frame: &composed,
                        mode: DisplayMode::Ascii,
                        theme: Theme::Monochrome,
                        truecolor: false,
                    },
                    frame.area(),
                );
            })
            .unwrap();
        assert!(
            terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .any(|cell| cell.symbol() != " ")
        );
    }
}
