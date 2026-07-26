use std::{
    io::{self, BufReader, IsTerminal},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};

use crate::{
    cli::{
        DemoArgs, DisplayMode, InputFormat, ReplayArgs, SchedulerArgs, StdinArgs, TcpArgs, Theme,
        ViewKind, VisualArgs,
    },
    ingestion::{Ingress, event_buffer},
    model::TemporalModel,
    recording::{Recorder, ReplaySource},
    render::GridWidget,
    source::{
        EventSource, demo::DemoSource, lines::LineSource, ndjson::NdjsonSource,
        tshark::TsharkTsvSource,
    },
    terminal::{TerminalSession, TerminationFlag},
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
        recorder: Option<Recorder>,
    },
    Replay {
        replay: ReplaySource,
    },
    #[cfg(all(target_os = "linux", feature = "ebpf"))]
    Scheduler {
        helper: crate::source::scheduler_helper::SchedulerHelper,
        recorder: Option<Recorder>,
    },
    #[cfg(all(target_os = "linux", feature = "ebpf"))]
    Tcp {
        helper: crate::source::tcp_helper::TcpHelper,
        recorder: Option<Recorder>,
    },
}

#[derive(Default)]
struct ProducerStats {
    malformed: AtomicU64,
    kernel_dropped: AtomicU64,
    collector_dropped: AtomicU64,
    ipc_dropped: AtomicU64,
}

pub fn run_demo(args: DemoArgs) -> Result<()> {
    if args.visual.snapshot || !io::stdout().is_terminal() {
        return snapshot_demo(&args);
    }
    run_interactive(Producer::Demo { seed: args.seed }, args.visual)
}

pub fn run_stdin(args: StdinArgs) -> Result<()> {
    if args.visual.snapshot || !io::stdout().is_terminal() {
        return snapshot_stdin(args);
    }
    let visual = args.visual.clone();
    let recorder = args.record.as_deref().map(Recorder::create).transpose()?;
    run_interactive(
        Producer::Stdin {
            format: args.format,
            recorder,
        },
        visual,
    )
}

pub fn run_replay(args: ReplayArgs) -> Result<()> {
    if args.visual.snapshot || !io::stdout().is_terminal() {
        return snapshot_replay(args);
    }
    let visual = args.visual.clone();
    let replay = ReplaySource::open(&args.path, args.speed)?;
    run_interactive(Producer::Replay { replay }, visual)
}

pub fn run_scheduler(args: SchedulerArgs) -> Result<()> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = args;
        Err(crate::source::scheduler::SchedulerSourceError::UnsupportedOperatingSystem.into())
    }
    #[cfg(all(target_os = "linux", not(feature = "ebpf")))]
    {
        let _ = args;
        Err(crate::source::scheduler::SchedulerSourceError::FeatureDisabled.into())
    }
    #[cfg(all(target_os = "linux", feature = "ebpf"))]
    {
        let helper = crate::source::scheduler_helper::SchedulerHelper::spawn()?;
        if args.visual.snapshot || !io::stdout().is_terminal() {
            return snapshot_scheduler(helper, args);
        }
        let recorder = args.record.as_deref().map(Recorder::create).transpose()?;
        run_interactive(Producer::Scheduler { helper, recorder }, args.visual)
    }
}

pub fn run_tcp(args: TcpArgs) -> Result<()> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = args;
        Err(crate::source::tcp::TcpSourceError::UnsupportedOperatingSystem.into())
    }
    #[cfg(all(target_os = "linux", not(feature = "ebpf")))]
    {
        let _ = args;
        Err(crate::source::tcp::TcpSourceError::FeatureDisabled.into())
    }
    #[cfg(all(target_os = "linux", feature = "ebpf"))]
    {
        let helper = crate::source::tcp_helper::TcpHelper::spawn()?;
        if args.visual.snapshot || !io::stdout().is_terminal() {
            return snapshot_tcp(helper, args);
        }
        let recorder = args.record.as_deref().map(Recorder::create).transpose()?;
        run_interactive(Producer::Tcp { helper, recorder }, args.visual)
    }
}

#[cfg(all(target_os = "linux", feature = "ebpf"))]
fn snapshot_tcp(mut helper: crate::source::tcp_helper::TcpHelper, args: TcpArgs) -> Result<()> {
    let mut recorder = args.record.as_deref().map(Recorder::create).transpose()?;
    let mut model = TemporalModel::default();
    let started = Instant::now();
    let deadline = started + Duration::from_millis(750);
    let mut losses = SnapshotLosses::default();
    while Instant::now() < deadline {
        match helper.next_pulse() {
            Ok(Some(pulse)) => {
                losses.kernel = pulse.kernel_ring_drops;
                losses.collector = pulse.collector_drops;
                losses.ipc = pulse.ipc_drops;
                if let Some(event) = pulse.into_normalized() {
                    if let Some(recorder) = &mut recorder {
                        recorder.record(&event)?;
                    }
                    model.ingest(event, started.elapsed().as_secs_f64());
                }
            }
            Ok(None) => {}
            Err(error) => return Err(error.into()),
        }
    }
    if let Some(recorder) = recorder {
        recorder.finish()?;
    }
    print_snapshot(&model, &args.visual, 0, losses)
}

#[cfg(all(target_os = "linux", feature = "ebpf"))]
fn snapshot_scheduler(
    mut helper: crate::source::scheduler_helper::SchedulerHelper,
    args: SchedulerArgs,
) -> Result<()> {
    let mut recorder = args.record.as_deref().map(Recorder::create).transpose()?;
    let mut model = TemporalModel::default();
    let started = Instant::now();
    let deadline = started + Duration::from_millis(750);
    let mut losses = SnapshotLosses::default();
    while Instant::now() < deadline {
        match helper.next_pulse() {
            Ok(Some(pulse)) => {
                losses.kernel = pulse.kernel_ring_drops;
                losses.collector = pulse.collector_drops;
                if let Some(event) = pulse.into_normalized() {
                    if let Some(recorder) = &mut recorder {
                        recorder.record(&event)?;
                    }
                    model.ingest(event, started.elapsed().as_secs_f64());
                }
            }
            Ok(None) => {}
            Err(error) => return Err(error.into()),
        }
    }
    if let Some(recorder) = recorder {
        recorder.finish()?;
    }
    print_snapshot(&model, &args.visual, 0, losses)
}

fn snapshot_demo(args: &DemoArgs) -> Result<()> {
    let mut model = TemporalModel::default();
    for (index, event) in DemoSource::new(args.seed).take(260).enumerate() {
        model.ingest(event, index as f64 * 0.035);
    }
    print_snapshot(&model, &args.visual, 0, SnapshotLosses::default())
}

fn snapshot_stdin(args: StdinArgs) -> Result<()> {
    let stdin = io::stdin();
    let mut source: Box<dyn EventSource> = match args.format {
        InputFormat::Lines => Box::new(LineSource::new(BufReader::new(stdin.lock()))),
        InputFormat::Ndjson => Box::new(NdjsonSource::new(BufReader::new(stdin.lock()))),
        InputFormat::TsharkTsv => Box::new(TsharkTsvSource::new(BufReader::new(stdin.lock()))),
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
    print_snapshot(
        &model,
        &args.visual,
        source.stats().malformed,
        SnapshotLosses::default(),
    )
}

fn snapshot_replay(args: ReplayArgs) -> Result<()> {
    let mut replay = ReplaySource::open(&args.path, args.speed)?;
    let mut model = TemporalModel::default();
    let mut now = 0.0;
    while let Some((delay, event)) = replay.next_timed()? {
        now += delay.as_secs_f64();
        model.ingest(event, now);
    }
    print_snapshot(
        &model,
        &args.visual,
        replay.stats().malformed,
        SnapshotLosses::default(),
    )
}

#[derive(Clone, Copy, Debug, Default)]
struct SnapshotLosses {
    kernel: u64,
    collector: u64,
    ipc: u64,
}

fn print_snapshot(
    model: &TemporalModel,
    visual: &VisualArgs,
    malformed: u64,
    losses: SnapshotLosses,
) -> Result<()> {
    let options = ViewOptions {
        mode: visual.mode,
        view: visual.view,
        gain: 1.0,
        paused: false,
        malformed,
        dropped: 0,
        kernel_dropped: losses.kernel,
        collector_dropped: losses.collector,
        ipc_dropped: losses.ipc,
        flight: None,
        help: false,
    };
    let frame = compose(&model.snapshot(), visual.width, visual.height, &options);
    println!("{}", frame.plain_text());
    Ok(())
}

fn run_interactive(producer: Producer, visual: VisualArgs) -> Result<()> {
    let termination = TerminationFlag::register()?;
    let (ingress, buffer) = event_buffer(CHANNEL_CAPACITY);
    let producer_stats = Arc::new(ProducerStats::default());
    let _producer_guard = spawn_producer(producer, ingress, Arc::clone(&producer_stats));

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
        if termination.requested() {
            break;
        }
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
        let width = size.width.min(500);
        let height = size.height.min(200);
        let options = ViewOptions {
            mode: state.mode,
            view: state.view,
            gain: state.gain,
            paused: state.paused,
            malformed: producer_stats.malformed.load(Ordering::Relaxed),
            dropped: buffer.dropped(),
            kernel_dropped: producer_stats.kernel_dropped.load(Ordering::Relaxed),
            collector_dropped: producer_stats.collector_dropped.load(Ordering::Relaxed),
            ipc_dropped: producer_stats.ipc_dropped.load(Ordering::Relaxed),
            flight: None,
            help: state.help,
        };
        let frame = compose(&model.snapshot(), width, height, &options);
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

struct ProducerGuard {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl Drop for ProducerGuard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn spawn_producer(
    producer: Producer,
    ingress: Ingress,
    stats: Arc<ProducerStats>,
) -> ProducerGuard {
    let stop = Arc::new(AtomicBool::new(false));
    #[cfg(all(target_os = "linux", feature = "ebpf"))]
    let worker_stop = Arc::clone(&stop);
    #[cfg(all(target_os = "linux", feature = "ebpf"))]
    let is_live_collector = matches!(&producer, Producer::Scheduler { .. } | Producer::Tcp { .. });
    #[cfg(not(all(target_os = "linux", feature = "ebpf")))]
    let is_live_collector = false;
    let join = thread::spawn(move || match producer {
        Producer::Demo { seed } => {
            for generated in DemoSource::new(seed) {
                if !ingress.submit(generated) {
                    thread::yield_now();
                }
                thread::sleep(Duration::from_millis(24));
            }
        }
        Producer::Stdin {
            format,
            mut recorder,
        } => {
            let stdin = io::stdin();
            let mut source: Box<dyn EventSource> = match format {
                InputFormat::Lines => Box::new(LineSource::new(BufReader::new(stdin.lock()))),
                InputFormat::Ndjson => Box::new(NdjsonSource::new(BufReader::new(stdin.lock()))),
                InputFormat::TsharkTsv => {
                    Box::new(TsharkTsvSource::new(BufReader::new(stdin.lock())))
                }
            };
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
        Producer::Replay { mut replay } => {
            while let Ok(Some((delay, incoming))) = replay.next_timed() {
                thread::sleep(delay);
                ingress.submit(incoming);
                stats
                    .malformed
                    .store(replay.stats().malformed, Ordering::Relaxed);
            }
        }
        #[cfg(all(target_os = "linux", feature = "ebpf"))]
        Producer::Scheduler {
            mut helper,
            mut recorder,
        } => {
            while !worker_stop.load(Ordering::Acquire) {
                match helper.next_pulse() {
                    Ok(Some(pulse)) => {
                        stats
                            .kernel_dropped
                            .store(pulse.kernel_ring_drops, Ordering::Relaxed);
                        stats
                            .collector_dropped
                            .store(pulse.collector_drops, Ordering::Relaxed);
                        if let Some(incoming) = pulse.into_normalized() {
                            if let Some(recorder) = &mut recorder {
                                let _ = recorder.record(&incoming);
                            }
                            ingress.submit(incoming);
                        }
                    }
                    Ok(None) => {}
                    Err(_) => {
                        stats.malformed.fetch_add(1, Ordering::Relaxed);
                        break;
                    }
                }
            }
            if let Some(recorder) = recorder {
                let _ = recorder.finish();
            }
        }
        #[cfg(all(target_os = "linux", feature = "ebpf"))]
        Producer::Tcp {
            mut helper,
            mut recorder,
        } => {
            while !worker_stop.load(Ordering::Acquire) {
                match helper.next_pulse() {
                    Ok(Some(pulse)) => {
                        stats
                            .kernel_dropped
                            .store(pulse.kernel_ring_drops, Ordering::Relaxed);
                        stats
                            .collector_dropped
                            .store(pulse.collector_drops, Ordering::Relaxed);
                        stats.ipc_dropped.store(pulse.ipc_drops, Ordering::Relaxed);
                        if let Some(incoming) = pulse.into_normalized() {
                            if let Some(recorder) = &mut recorder {
                                let _ = recorder.record(&incoming);
                            }
                            ingress.submit(incoming);
                        }
                    }
                    Ok(None) => {}
                    Err(_) => {
                        stats.malformed.fetch_add(1, Ordering::Relaxed);
                        break;
                    }
                }
            }
            if let Some(recorder) = recorder {
                let _ = recorder.finish();
            }
        }
    });
    ProducerGuard {
        stop,
        join: is_live_collector.then_some(join),
    }
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
            kernel_dropped: 0,
            collector_dropped: 0,
            ipc_dropped: 0,
            flight: None,
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
