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
        DemoArgs, DisplayMode, InputFormat, ParticleMode, ProcArgs, ReplayArgs, SchedulerArgs,
        StdinArgs, TcpArgs, Theme, ViewKind, VisualArgs,
    },
    flight_runtime::FlightRuntime,
    ingestion::{Ingress, event_buffer},
    model::TemporalModel,
    recording::{Recorder, ReplaySource},
    render::{ColorDepth, GridWidget},
    source::{
        EventSource, demo::DemoSource, lines::LineSource, ndjson::NdjsonSource,
        tshark::TsharkTsvSource,
    },
    terminal::{TerminalSession, TerminationFlag},
    view::{ProcSourceStatus, ViewOptions, compose},
};

#[cfg(all(target_os = "linux", feature = "ebpf"))]
use crate::{
    cli::FlightArgs,
    flight_recorder::{
        DEFAULT_POST_TRIGGER, DEFAULT_PRE_TRIGGER, FlightConfig, FlightSource, IncidentLosses,
    },
    flight_runtime::FlightRuntimeSender,
    trigger::TriggerSpec,
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
    #[cfg(target_os = "linux")]
    Proc {
        source: crate::source::procfs::LiveProcSource,
        interval: Duration,
        recorder: Option<Recorder>,
    },
    #[cfg(all(target_os = "linux", feature = "ebpf"))]
    Scheduler {
        helper: crate::source::scheduler_helper::SchedulerHelper,
        recorder: Option<Recorder>,
        flight: Option<FlightRuntimeSender>,
    },
    #[cfg(all(target_os = "linux", feature = "ebpf"))]
    Tcp {
        helper: crate::source::tcp_helper::TcpHelper,
        recorder: Option<Recorder>,
        flight: Option<FlightRuntimeSender>,
    },
}

#[derive(Default)]
struct ProducerStats {
    malformed: AtomicU64,
    kernel_dropped: AtomicU64,
    collector_dropped: AtomicU64,
    ipc_dropped: AtomicU64,
    renderer_dropped: AtomicU64,
    proc_interval_ms: AtomicU64,
    proc_tracked: AtomicU64,
    proc_unreadable: AtomicU64,
    proc_vanished: AtomicU64,
    proc_source_dropped: AtomicU64,
    proc_failed: AtomicBool,
}

pub fn run_demo(args: DemoArgs) -> Result<()> {
    if args.visual.snapshot || !io::stdout().is_terminal() {
        return snapshot_demo(&args);
    }
    run_interactive(Producer::Demo { seed: args.seed }, args.visual, None)
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
        None,
    )
}

pub fn run_replay(args: ReplayArgs) -> Result<()> {
    if args.visual.snapshot || !io::stdout().is_terminal() {
        return snapshot_replay(args);
    }
    let visual = args.visual.clone();
    let replay = ReplaySource::open(&args.path, args.speed)?;
    run_interactive(Producer::Replay { replay }, visual, None)
}

pub fn run_proc(args: ProcArgs) -> Result<()> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = args;
        Err(crate::source::procfs::ProcSourceError::UnsupportedOperatingSystem.into())
    }
    #[cfg(target_os = "linux")]
    {
        let source = crate::source::procfs::LiveProcSource::new(
            args.pid,
            args.anonymize,
            session_seed(),
            args.net_interface.clone(),
        );
        if args.visual.snapshot || !io::stdout().is_terminal() {
            return snapshot_proc(source, args);
        }
        let visual = args.visual.clone();
        let recorder = args.record.as_deref().map(Recorder::create).transpose()?;
        run_interactive(
            Producer::Proc {
                source,
                interval: args.interval,
                recorder,
            },
            visual,
            None,
        )
    }
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
        if args.flight.flight_recorder.is_some()
            && (args.visual.snapshot || !io::stdout().is_terminal())
        {
            return Err(anyhow::anyhow!(
                "flight recording requires an interactive terminal; replay the completed incident for snapshots"
            ));
        }
        let flight = flight_runtime(&args.flight, FlightSource::Scheduler)?;
        let flight_sender = flight.as_ref().map(FlightRuntime::sender);
        let helper = crate::source::scheduler_helper::SchedulerHelper::spawn()?;
        if args.visual.snapshot || !io::stdout().is_terminal() {
            return snapshot_scheduler(helper, args);
        }
        let recorder = args.record.as_deref().map(Recorder::create).transpose()?;
        run_interactive(
            Producer::Scheduler {
                helper,
                recorder,
                flight: flight_sender,
            },
            args.visual,
            flight,
        )
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
        if args.flight.flight_recorder.is_some()
            && (args.visual.snapshot || !io::stdout().is_terminal())
        {
            return Err(anyhow::anyhow!(
                "flight recording requires an interactive terminal; replay the completed incident for snapshots"
            ));
        }
        let flight = flight_runtime(&args.flight, FlightSource::Tcp)?;
        let flight_sender = flight.as_ref().map(FlightRuntime::sender);
        let helper = crate::source::tcp_helper::TcpHelper::spawn()?;
        if args.visual.snapshot || !io::stdout().is_terminal() {
            return snapshot_tcp(helper, args);
        }
        let recorder = args.record.as_deref().map(Recorder::create).transpose()?;
        run_interactive(
            Producer::Tcp {
                helper,
                recorder,
                flight: flight_sender,
            },
            args.visual,
            flight,
        )
    }
}

#[cfg(all(target_os = "linux", feature = "ebpf"))]
fn flight_runtime(args: &FlightArgs, source: FlightSource) -> Result<Option<FlightRuntime>> {
    let Some(output) = &args.flight_recorder else {
        if args.pre_trigger != DEFAULT_PRE_TRIGGER
            || args.post_trigger != DEFAULT_POST_TRIGGER
            || args.trigger != TriggerSpec::Auto
        {
            return Err(anyhow::anyhow!(
                "--pre-trigger, --post-trigger, and --trigger require --flight-recorder"
            ));
        }
        return Ok(None);
    };
    let mut config = FlightConfig::new(output.clone(), source);
    config.pre_trigger = args.pre_trigger;
    config.post_trigger = args.post_trigger;
    Ok(Some(FlightRuntime::spawn(config, args.trigger.clone())?))
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

#[cfg(target_os = "linux")]
fn snapshot_proc(mut source: crate::source::procfs::LiveProcSource, args: ProcArgs) -> Result<()> {
    let mut recorder = args.record.as_deref().map(Recorder::create).transpose()?;
    let mut model = TemporalModel::default();
    let started = Instant::now();
    for event in source.sample(0.0)? {
        model.ingest(event, 0.0);
    }
    for _ in 0..3 {
        thread::sleep(args.interval);
        let now = started.elapsed().as_secs_f64();
        for event in source.sample(now)? {
            if let Some(recorder) = &mut recorder {
                recorder.record(&event)?;
            }
            model.ingest(event, now);
        }
    }
    if let Some(recorder) = recorder {
        recorder.finish()?;
    }
    let stats = source.stats();
    print_snapshot(
        &model,
        &args.visual,
        0,
        SnapshotLosses {
            proc: Some(proc_source_status(args.interval, stats)),
            ..SnapshotLosses::default()
        },
    )
}

#[derive(Clone, Copy, Debug, Default)]
struct SnapshotLosses {
    kernel: u64,
    collector: u64,
    ipc: u64,
    proc: Option<ProcSourceStatus>,
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
        meter_scale: visual.meter_scale,
        gain: 1.0,
        paused: false,
        malformed,
        dropped: 0,
        kernel_dropped: losses.kernel,
        collector_dropped: losses.collector,
        ipc_dropped: losses.ipc,
        flight: None,
        proc: losses.proc,
        particles: visual.particles == ParticleMode::On,
        help: false,
    };
    let frame = compose(&model.snapshot(), visual.width, visual.height, &options);
    println!("{}", frame.plain_text());
    Ok(())
}

fn run_interactive(
    producer: Producer,
    visual: VisualArgs,
    flight: Option<FlightRuntime>,
) -> Result<()> {
    let termination = TerminationFlag::register()?;
    let (ingress, buffer) = event_buffer(CHANNEL_CAPACITY);
    let producer_stats = Arc::new(ProducerStats::default());
    let _producer_guard = spawn_producer(producer, ingress, Arc::clone(&producer_stats));

    let mut session = TerminalSession::enter()?;
    let depth = terminal_color_depth();
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
        particles: visual.particles == ParticleMode::On,
        help: false,
    };
    let mut model = TemporalModel::new(state.decay);
    let started = Instant::now();
    let mut drained = Vec::with_capacity(MAX_DRAIN_PER_FRAME);

    let outcome = (|| -> Result<()> {
        loop {
            if termination.requested() {
                if let Some(flight) = &flight {
                    let _ = flight.interrupt();
                }
                break;
            }
            if let Some(flight) = &flight {
                if let Some(error) = flight.error() {
                    return Err(anyhow::anyhow!("flight recorder failed: {error}"));
                }
                if flight.published() {
                    break;
                }
            }
            if producer_stats.proc_failed.load(Ordering::Acquire) {
                return Err(anyhow::anyhow!(
                    "the proc activity sampler stopped because /proc host state could not be read or parsed"
                ));
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
                meter_scale: visual.meter_scale,
                gain: state.gain,
                paused: state.paused,
                malformed: producer_stats.malformed.load(Ordering::Relaxed),
                dropped: buffer.dropped(),
                kernel_dropped: producer_stats.kernel_dropped.load(Ordering::Relaxed),
                collector_dropped: producer_stats.collector_dropped.load(Ordering::Relaxed),
                ipc_dropped: producer_stats.ipc_dropped.load(Ordering::Relaxed),
                flight: flight.as_ref().map(FlightRuntime::status),
                proc: {
                    let interval_ms = producer_stats.proc_interval_ms.load(Ordering::Relaxed);
                    (interval_ms > 0).then(|| ProcSourceStatus {
                        interval_ms,
                        tracked: producer_stats.proc_tracked.load(Ordering::Relaxed),
                        unreadable: producer_stats.proc_unreadable.load(Ordering::Relaxed),
                        vanished: producer_stats.proc_vanished.load(Ordering::Relaxed),
                        source_dropped: producer_stats.proc_source_dropped.load(Ordering::Relaxed),
                    })
                },
                particles: state.particles,
                help: state.help,
            };
            let frame = compose(&model.snapshot(), width, height, &options);
            session.terminal_mut().draw(|terminal_frame| {
                terminal_frame.render_widget(
                    GridWidget {
                        frame: &frame,
                        mode: state.mode,
                        theme: state.theme,
                        depth,
                    },
                    terminal_frame.area(),
                );
            })?;

            let remaining = FRAME_TIME.saturating_sub(frame_started.elapsed());
            if event::poll(remaining)? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        if key.code == KeyCode::Char('t') {
                            if let Some(flight) = &flight {
                                if flight.status().state
                                    == crate::flight_recorder::FlightState::Armed
                                {
                                    flight.manual_trigger()?;
                                }
                            }
                            continue;
                        }
                        if key.code == KeyCode::Char('x') {
                            if let Some(flight) = &flight {
                                if flight.status().state
                                    == crate::flight_recorder::FlightState::Armed
                                {
                                    flight.cancel()?;
                                }
                            }
                            continue;
                        }
                        if handle_key(key.code, key.modifiers, &mut state, ansi_allowed) {
                            if let Some(flight) = &flight {
                                let _ = flight.interrupt();
                            }
                            break;
                        }
                        model.set_decay(state.decay);
                    }
                    Event::Resize(_, _) => {}
                    _ => {}
                }
            }
            producer_stats
                .renderer_dropped
                .store(buffer.dropped(), Ordering::Relaxed);
        }
        Ok(())
    })();
    if outcome.is_err() {
        if let Some(flight) = &flight {
            let _ = flight.renderer_failed();
        }
    }
    outcome
}

struct UiState {
    mode: DisplayMode,
    view: ViewKind,
    theme: Theme,
    gain: f32,
    decay: f64,
    paused: bool,
    particles: bool,
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
        KeyCode::Char('3') => state.view = ViewKind::Meter,
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
                Theme::Monochrome => Theme::Rainbow,
                Theme::Rainbow => Theme::Pastel,
                Theme::Pastel => Theme::Phosphor,
            };
        }
        KeyCode::Char('+') | KeyCode::Char('=') => state.gain = (state.gain * 1.15).min(4.0),
        KeyCode::Char('-') => state.gain = (state.gain / 1.15).max(0.2),
        KeyCode::Char('[') => state.decay = (state.decay / 1.15).max(0.2),
        KeyCode::Char(']') => state.decay = (state.decay * 1.15).min(30.0),
        KeyCode::Char('p') => state.particles = !state.particles,
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
    #[cfg(target_os = "linux")]
    let worker_stop = Arc::clone(&stop);
    let needs_join = match &producer {
        #[cfg(target_os = "linux")]
        Producer::Proc { .. } => true,
        #[cfg(all(target_os = "linux", feature = "ebpf"))]
        Producer::Scheduler { .. } | Producer::Tcp { .. } => true,
        _ => false,
    };
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
        #[cfg(target_os = "linux")]
        Producer::Proc {
            mut source,
            interval,
            mut recorder,
        } => {
            stats
                .proc_interval_ms
                .store(interval.as_millis() as u64, Ordering::Relaxed);
            let started = Instant::now();
            while !worker_stop.load(Ordering::Acquire) {
                match source.sample(started.elapsed().as_secs_f64()) {
                    Ok(events) => {
                        for incoming in events {
                            if let Some(recorder) = &mut recorder {
                                let _ = recorder.record(&incoming);
                            }
                            ingress.submit(incoming);
                        }
                        update_proc_stats(&stats, interval, source.stats());
                    }
                    Err(_) => {
                        stats.proc_failed.store(true, Ordering::Release);
                        break;
                    }
                }
                interruptible_sleep(interval, &worker_stop);
            }
            if let Some(recorder) = recorder {
                let _ = recorder.finish();
            }
        }
        #[cfg(all(target_os = "linux", feature = "ebpf"))]
        Producer::Scheduler {
            mut helper,
            mut recorder,
            flight,
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
                        if let Some(flight) = &flight {
                            flight.update_losses(incident_losses(&stats));
                        }
                        if let Some(incoming) = pulse.into_normalized() {
                            if let Some(flight) = &flight {
                                flight.observe(incoming.clone(), incident_losses(&stats));
                            }
                            if let Some(recorder) = &mut recorder {
                                let _ = recorder.record(&incoming);
                            }
                            ingress.submit(incoming);
                        }
                    }
                    Ok(None) => {}
                    Err(_) => {
                        stats.malformed.fetch_add(1, Ordering::Relaxed);
                        if let Some(flight) = &flight {
                            flight.source_failed();
                        }
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
            flight,
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
                        if let Some(flight) = &flight {
                            flight.update_losses(incident_losses(&stats));
                        }
                        if let Some(incoming) = pulse.into_normalized() {
                            if let Some(flight) = &flight {
                                flight.observe(incoming.clone(), incident_losses(&stats));
                            }
                            if let Some(recorder) = &mut recorder {
                                let _ = recorder.record(&incoming);
                            }
                            ingress.submit(incoming);
                        }
                    }
                    Ok(None) => {}
                    Err(_) => {
                        stats.malformed.fetch_add(1, Ordering::Relaxed);
                        if let Some(flight) = &flight {
                            flight.source_failed();
                        }
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
        join: needs_join.then_some(join),
    }
}

#[cfg(target_os = "linux")]
fn proc_source_status(
    interval: Duration,
    stats: crate::source::procfs::ProcStats,
) -> ProcSourceStatus {
    ProcSourceStatus {
        interval_ms: interval.as_millis() as u64,
        tracked: stats.tracked_processes as u64,
        unreadable: stats.unreadable,
        vanished: stats.vanished,
        source_dropped: stats
            .scan_refused
            .saturating_add(stats.tracking_refused)
            .saturating_add(stats.event_refused),
    }
}

#[cfg(target_os = "linux")]
fn update_proc_stats(
    destination: &ProducerStats,
    interval: Duration,
    source: crate::source::procfs::ProcStats,
) {
    let status = proc_source_status(interval, source);
    destination
        .proc_tracked
        .store(status.tracked, Ordering::Relaxed);
    destination
        .proc_unreadable
        .store(status.unreadable, Ordering::Relaxed);
    destination
        .proc_vanished
        .store(status.vanished, Ordering::Relaxed);
    destination
        .proc_source_dropped
        .store(status.source_dropped, Ordering::Relaxed);
}

#[cfg(target_os = "linux")]
fn interruptible_sleep(duration: Duration, stop: &AtomicBool) {
    let deadline = Instant::now() + duration;
    while !stop.load(Ordering::Acquire) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        thread::sleep(remaining.min(Duration::from_millis(25)));
    }
}

#[cfg(target_os = "linux")]
fn session_seed() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos() as u64)
        ^ u64::from(std::process::id())
}

#[cfg(all(target_os = "linux", feature = "ebpf"))]
fn incident_losses(stats: &ProducerStats) -> IncidentLosses {
    IncidentLosses {
        kernel_ring: stats.kernel_dropped.load(Ordering::Relaxed),
        collector: stats.collector_dropped.load(Ordering::Relaxed),
        ipc: stats.ipc_dropped.load(Ordering::Relaxed),
        renderer_channel: stats.renderer_dropped.load(Ordering::Relaxed),
        malformed: stats.malformed.load(Ordering::Relaxed),
        writer: 0,
    }
}

fn terminal_color() -> bool {
    std::env::var_os("NO_COLOR").is_none()
        && std::env::var("TERM").map_or(true, |term| term != "dumb")
}

fn terminal_color_depth() -> ColorDepth {
    if terminal_color()
        && std::env::var("COLORTERM")
            .is_ok_and(|value| value.contains("truecolor") || value.contains("24bit"))
    {
        ColorDepth::Truecolor
    } else if std::env::var("TERM").is_ok_and(|term| term.contains("256color")) {
        ColorDepth::Xterm256
    } else {
        ColorDepth::Ansi16
    }
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
            meter_scale: crate::cli::MeterScale::Perceptual,
            gain: 1.0,
            paused: false,
            malformed: 0,
            dropped: 0,
            kernel_dropped: 0,
            collector_dropped: 0,
            ipc_dropped: 0,
            flight: None,
            proc: None,
            particles: true,
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
                        depth: ColorDepth::Ansi16,
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
