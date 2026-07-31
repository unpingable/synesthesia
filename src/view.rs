use crate::{
    cli::{DisplayMode, ViewKind},
    event::Direction,
    flight_recorder::{FlightState, FlightStatus},
    model::{Activity, ModelSnapshot, Particle, ParticleStyle, ProcMetrics, TcpMetrics},
    render::{Cell, RenderFrame},
    source::stable_hash,
};

const ASCII_DENSITY: &[u8] = b" .:-=+*#%@";
const ANSI_DENSITY: &[char] = &[' ', '·', '░', '▒', '▓', '█'];

#[derive(Clone, Debug)]
pub struct ViewOptions {
    pub mode: DisplayMode,
    pub view: ViewKind,
    pub gain: f32,
    pub paused: bool,
    pub malformed: u64,
    pub dropped: u64,
    pub kernel_dropped: u64,
    pub collector_dropped: u64,
    pub ipc_dropped: u64,
    pub flight: Option<FlightStatus>,
    pub proc: Option<ProcSourceStatus>,
    pub particles: bool,
    pub help: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProcSourceStatus {
    pub interval_ms: u64,
    pub tracked: u64,
    pub unreadable: u64,
    pub vanished: u64,
    pub source_dropped: u64,
}

pub fn compose(
    snapshot: &ModelSnapshot,
    width: u16,
    height: u16,
    options: &ViewOptions,
) -> RenderFrame {
    let mut frame = RenderFrame::new(width, height);
    let field_height = height.saturating_sub(1);
    if width > 0 && field_height > 0 {
        match options.view {
            ViewKind::Weather => weather(snapshot, &mut frame, field_height, options),
            ViewKind::Waterfall => waterfall(snapshot, &mut frame, field_height, options),
            ViewKind::Meter => meter(snapshot, &mut frame, field_height, options),
        }
        if options.particles {
            particle_overlay(snapshot, &mut frame, field_height, options);
        }
    }
    let mut status = if options.help {
        if options.flight.is_some() {
            " t trigger  x cancel  q/esc cancel-or-finalize  space pause  1/2/3 view  a mode  c theme  p particles "
                .to_owned()
        } else {
            " q/esc quit  space pause  1 weather  2 waterfall  3 meter  a ascii/ansi  c theme  p particles  +/- gain  [] decay "
                .to_owned()
        }
    } else if let Some(tcp) = &snapshot.metrics.tcp {
        tcp_status(snapshot, tcp, width, options)
    } else if let Some(scheduler) = &snapshot.metrics.scheduler {
        scheduler_status(snapshot, scheduler, width, options)
    } else if let Some(proc) = &snapshot.metrics.proc {
        proc_status(snapshot, proc, width, options)
    } else if options.proc.is_some() {
        proc_status(snapshot, &ProcMetrics::default(), width, options)
    } else {
        format!(
            " {:>5.1} evt/s  {:>7}/s  {:>3} flows  bad {} drop {}  {:?}/{:?}{}",
            clean_zero(snapshot.metrics.events_per_second),
            compact_magnitude(snapshot.metrics.magnitude_per_second),
            snapshot.metrics.active_flows,
            options.malformed,
            options.dropped,
            options.view,
            options.mode,
            if options.paused { " PAUSED" } else { "" }
        )
        .to_lowercase()
    };
    if let Some(flight) = &options.flight {
        status = format!(" {} |{}", flight_status(flight), status);
    } else if let Some(flight) = &snapshot.metrics.flight {
        status = format!(
            " replay {} {} {} loss {}/{}/{}/{}/{} |{}",
            flight.phase.to_uppercase(),
            flight.source.as_deref().unwrap_or("incident"),
            flight.trigger_kind.as_deref().unwrap_or("armed"),
            flight.losses.kernel_ring,
            flight.losses.collector,
            flight.losses.ipc,
            flight.losses.renderer_channel,
            flight.losses.writer,
            status
        );
    }
    frame.write_status(&status);
    frame
}

fn proc_status(
    snapshot: &ModelSnapshot,
    proc: &ProcMetrics,
    width: u16,
    options: &ViewOptions,
) -> String {
    let state = if options.paused { " pause" } else { "" };
    let source = options.proc.unwrap_or_default();
    let interval = options.proc.map_or_else(
        || "replay".to_owned(),
        |status| format!("{}ms", status.interval_ms),
    );
    if width < 110 {
        format!(
            " {} proc/s cpu {} io {} +/- {}/{} active {} {} miss {}/{} drop {}/{} {}/{}{}",
            compact_magnitude(snapshot.metrics.events_per_second),
            compact_magnitude(proc.cpu_events_per_second),
            compact_magnitude(proc.io_events_per_second),
            compact_magnitude(proc.starts_per_second),
            compact_magnitude(proc.exits_per_second),
            source.tracked.max(proc.active_processes as u64),
            interval,
            source.unreadable,
            source.vanished,
            source.source_dropped,
            options.dropped,
            short_view(options.view),
            short_mode(options.mode),
            state,
        )
    } else {
        format!(
            " {} proc/s  {} cpu  {} io  start/exit {}/{}  {} active  {}  unread/vanish {}/{}  drop source/user {}/{}  {:?}/{:?}{}",
            compact_magnitude(snapshot.metrics.events_per_second),
            compact_magnitude(proc.cpu_events_per_second),
            compact_magnitude(proc.io_events_per_second),
            compact_magnitude(proc.starts_per_second),
            compact_magnitude(proc.exits_per_second),
            source.tracked.max(proc.active_processes as u64),
            interval,
            source.unreadable,
            source.vanished,
            source.source_dropped,
            options.dropped,
            options.view,
            options.mode,
            state,
        )
        .to_lowercase()
    }
}

fn particle_overlay(
    snapshot: &ModelSnapshot,
    frame: &mut RenderFrame,
    field_height: u16,
    options: &ViewOptions,
) {
    let width = f64::from(frame.width.saturating_sub(1));
    let height = f64::from(field_height.saturating_sub(1));
    for particle in &snapshot.particles {
        let age = (snapshot.now - particle.born).max(0.0);
        if age > particle.lifetime {
            continue;
        }
        let life = (1.0 - age / particle.lifetime).clamp(0.0, 1.0);
        let x = particle.origin_x + particle.velocity_x * age;
        let y = (particle.origin_y + particle.velocity_y * age).rem_euclid(1.0);
        let intensity = (particle.energy * (life * life) as f32 * options.gain).clamp(0.04, 0.92);
        frame.put_overlay(
            (x * width).round() as i32,
            (y * height).round() as i32,
            particle_cell(particle, intensity, options.mode),
        );
    }
}

fn particle_cell(particle: &Particle, intensity: f32, mode: DisplayMode) -> Cell {
    let phase = ((particle.seed >> 17) & 3) as usize;
    let glyph = match (mode, particle.style) {
        (DisplayMode::Ascii, ParticleStyle::Ember) => ['.', ',', '\'', '`'][phase],
        (DisplayMode::Ascii, ParticleStyle::Fracture) => ['/', '\\', '*', '+'][phase],
        (DisplayMode::Ascii, ParticleStyle::Impact) => ['x', '*', '+', 'x'][phase],
        (DisplayMode::Ascii, ParticleStyle::Migration) => match particle.direction {
            Direction::Inbound => '<',
            Direction::Outbound => '>',
            Direction::Neutral | Direction::Unknown => '+',
        },
        (DisplayMode::Ansi, ParticleStyle::Ember) => ['·', '∙', '•', '✦'][phase],
        (DisplayMode::Ansi, ParticleStyle::Fracture) => ['╱', '╲', '✦', '∗'][phase],
        (DisplayMode::Ansi, ParticleStyle::Impact) => ['◆', '✕', '✦', '◇'][phase],
        (DisplayMode::Ansi, ParticleStyle::Migration) => match particle.direction {
            Direction::Inbound => '◂',
            Direction::Outbound => '▸',
            Direction::Neutral | Direction::Unknown => '◆',
        },
    };
    Cell {
        glyph,
        intensity,
        category: particle.category,
        direction: particle.direction,
    }
}

fn clean_zero(value: f64) -> f64 {
    if value.abs() < 0.05 { 0.0 } else { value }
}

fn flight_status(status: &FlightStatus) -> String {
    match status.state {
        FlightState::Disarmed => "rec off".to_owned(),
        FlightState::Armed => format!(
            "rec armed pre {:.1}s evict {}",
            status.retained_duration, status.pre_trigger_evictions
        ),
        FlightState::CapturingTail => format!(
            "rec triggered {} tail {:.1}/{:.1}s",
            status.trigger_kind.as_deref().unwrap_or("unknown"),
            status.tail_elapsed.min(status.tail_duration),
            status.tail_duration
        ),
        FlightState::Complete => format!("rec saved {}", short_path(&status.output)),
        FlightState::Cancelled => "rec cancelled".to_owned(),
        FlightState::Failed => "rec failed".to_owned(),
    }
}

fn short_path(path: &std::path::Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("incident.ndjson")
        .chars()
        .take(28)
        .collect()
}

fn tcp_status(
    snapshot: &ModelSnapshot,
    tcp: &TcpMetrics,
    width: u16,
    options: &ViewOptions,
) -> String {
    let state = if options.paused { " pause" } else { "" };
    if width < 110 {
        format!(
            " {} tcp/s rt {} tx/rx {}/{} flow {} loss {}/{}/{}/{} {}/{}{}",
            compact_magnitude(snapshot.metrics.events_per_second),
            compact_magnitude(tcp.retransmits_per_second),
            compact_magnitude(tcp.resets_sent_per_second),
            compact_magnitude(tcp.resets_received_per_second),
            tcp.active_pathological_flows,
            options.kernel_dropped,
            options.collector_dropped,
            options.ipc_dropped,
            options.dropped,
            short_view(options.view),
            short_mode(options.mode),
            state,
        )
    } else {
        format!(
            " {} tcp/s  {} retransmit  reset tx/rx {}/{}  {} flows  loss k/c/i/u {}/{}/{}/{}  {:?}/{:?}{}",
            compact_magnitude(snapshot.metrics.events_per_second),
            compact_magnitude(tcp.retransmits_per_second),
            compact_magnitude(tcp.resets_sent_per_second),
            compact_magnitude(tcp.resets_received_per_second),
            tcp.active_pathological_flows,
            options.kernel_dropped,
            options.collector_dropped,
            options.ipc_dropped,
            options.dropped,
            options.view,
            options.mode,
            state,
        )
        .to_lowercase()
    }
}

fn scheduler_status(
    snapshot: &ModelSnapshot,
    scheduler: &crate::model::SchedulerMetrics,
    width: u16,
    options: &ViewOptions,
) -> String {
    let state = if options.paused { " pause" } else { "" };
    if width < 100 {
        format!(
            " {} sched/s sw {} wk {} mg {} cpu {} loss {}/{}/{} {}/{}{}",
            compact_magnitude(snapshot.metrics.events_per_second),
            compact_magnitude(scheduler.switches_per_second),
            compact_magnitude(scheduler.wakeups_per_second),
            compact_magnitude(scheduler.migrations_per_second),
            scheduler.active_cpus,
            options.kernel_dropped,
            options.collector_dropped,
            options.dropped,
            short_view(options.view),
            short_mode(options.mode),
            state,
        )
    } else {
        format!(
            " {} sched/s  {} switch  {} wake  {} migrate  {} cpu  loss k/c/u {}/{}/{}  {:?}/{:?}{}",
            compact_magnitude(snapshot.metrics.events_per_second),
            compact_magnitude(scheduler.switches_per_second),
            compact_magnitude(scheduler.wakeups_per_second),
            compact_magnitude(scheduler.migrations_per_second),
            scheduler.active_cpus,
            options.kernel_dropped,
            options.collector_dropped,
            options.dropped,
            options.view,
            options.mode,
            state,
        )
        .to_lowercase()
    }
}

fn short_view(view: ViewKind) -> &'static str {
    match view {
        ViewKind::Weather => "w",
        ViewKind::Waterfall => "f",
        ViewKind::Meter => "m",
    }
}

fn short_mode(mode: DisplayMode) -> &'static str {
    match mode {
        DisplayMode::Ansi => "ansi",
        DisplayMode::Ascii => "ascii",
    }
}

fn weather(
    snapshot: &ModelSnapshot,
    frame: &mut RenderFrame,
    field_height: u16,
    options: &ViewOptions,
) {
    let width = f64::from(frame.width);
    let height = f64::from(field_height);
    let time_tick = (snapshot.now * 8.0).floor() as u64;
    for y in 0..field_height {
        for x in 0..frame.width {
            let weather_hash = u64::from(x).wrapping_mul(0x9e37)
                ^ u64::from(y).wrapping_mul(0x85eb)
                ^ (time_tick / 3);
            if weather_hash % 113 == 0 {
                frame.put(
                    i32::from(x),
                    i32::from(y),
                    visual_cell(0.08, weather_hash, Direction::Neutral, options.mode),
                );
            }
        }
    }

    for activity in &snapshot.activity {
        let age = (snapshot.now - activity.born).max(0.0);
        let life = (1.0 - age / (snapshot.decay_seconds * 2.5)).clamp(0.0, 1.0);
        if life <= 0.0 {
            continue;
        }
        if activity.category == stable_hash(crate::flight_recorder::TRIGGER_CATEGORY.as_bytes()) {
            trigger_marker(frame, field_height, life as f32, options.mode);
            continue;
        }
        if let Some(pathology) = tcp_visual(activity.category) {
            tcp_weather(
                activity,
                pathology,
                age,
                life,
                snapshot,
                frame,
                field_height,
                options,
            );
            continue;
        }
        let weight = ((activity.magnitude + 1.0).log2() / 12.0).clamp(0.12, 1.35);
        let intensity = (life * weight * f64::from(options.gain)).clamp(0.03, 1.0) as f32;
        let phase = age / snapshot.decay_seconds;
        let anchor = (activity.flow % 10_000) as f64 / 10_000.0;
        let drift = width * 0.48 * phase;
        let x = match activity.direction {
            Direction::Outbound => width * 0.08 + width * anchor * 0.28 + drift,
            Direction::Inbound => width * 0.92 - width * anchor * 0.28 - drift,
            Direction::Neutral => {
                width * (0.22 + anchor * 0.56) + (phase * 7.0 + anchor * 9.0).sin() * width * 0.06
            }
            Direction::Unknown => {
                width * anchor + (phase * 4.0 + anchor * 5.0).cos() * width * 0.04
            }
        };
        let lane = (activity.lane % u64::from(field_height)) as f64;
        let y = (lane + (phase * 5.5 + (activity.flow % 31) as f64).sin() * height * 0.08)
            .rem_euclid(height);
        let velocity = match activity.direction {
            Direction::Inbound => -1.0,
            Direction::Outbound => 1.0,
            Direction::Neutral | Direction::Unknown => 0.35,
        };
        let trail = (3.0 + weight * 7.0) as i32;
        let category = activity.category;
        for step in 0..trail {
            let fade = 1.0 - step as f32 / trail as f32;
            let trail_x = x - f64::from(step) * velocity;
            let trail_y = y + (f64::from(step) * 0.32 + anchor * 8.0).sin() * 0.7;
            frame.put(
                trail_x.round() as i32,
                trail_y.round() as i32,
                visual_cell(intensity * fade, category, activity.direction, options.mode),
            );
        }
        if activity.magnitude > 1_200.0 {
            for radius in 1..=((weight * 3.0) as i32) {
                let echo = intensity * (0.65 / radius as f32);
                for dy in [-radius, radius] {
                    frame.put(
                        x.round() as i32,
                        y.round() as i32 + dy,
                        visual_cell(echo, category, activity.direction, options.mode),
                    );
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TcpVisual {
    Retransmit,
    ResetSent,
    ResetReceived,
}

fn tcp_visual(category: u64) -> Option<TcpVisual> {
    if category == stable_hash(b"tcp.retransmit") {
        Some(TcpVisual::Retransmit)
    } else if category == stable_hash(b"tcp.reset.send") {
        Some(TcpVisual::ResetSent)
    } else if category == stable_hash(b"tcp.reset.receive") {
        Some(TcpVisual::ResetReceived)
    } else {
        None
    }
}

#[allow(clippy::too_many_arguments)]
fn tcp_weather(
    activity: &Activity,
    pathology: TcpVisual,
    age: f64,
    life: f64,
    snapshot: &ModelSnapshot,
    frame: &mut RenderFrame,
    field_height: u16,
    options: &ViewOptions,
) {
    let width = f64::from(frame.width);
    let height = f64::from(field_height);
    let anchor = (activity.flow % 10_000) as f64 / 10_000.0;
    let phase = age / snapshot.decay_seconds;
    let direction = match activity.direction {
        Direction::Inbound => -1.0,
        _ => 1.0,
    };
    let center_x = width * (0.18 + anchor * 0.64) + direction * phase.min(1.5) * width * 0.08;
    let center_y = (activity.lane % u64::from(field_height)) as f64
        + (phase * 3.0 + anchor * 7.0).sin() * height * 0.025;
    let weight = ((activity.magnitude + 1.0).log2() / 14.0).clamp(0.55, 1.25);
    let intensity = (life * weight * f64::from(options.gain)).clamp(0.08, 1.0) as f32;

    match pathology {
        TcpVisual::Retransmit => {
            let length = (9.0 + weight * 15.0).round() as i32;
            let mut previous_y = center_y.round() as i32;
            for step in 0..length {
                let centered = step - length / 2;
                let x = center_x.round() as i32 + (f64::from(centered) * direction).round() as i32;
                let fracture = activity
                    .flow
                    .wrapping_add((step as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15));
                let y = center_y.round() as i32 + (fracture % 5) as i32 - 2;
                let fade = 1.0 - (centered.unsigned_abs() as f32 / length as f32) * 0.55;
                let glyph = if y > previous_y {
                    '\\'
                } else if y < previous_y {
                    '/'
                } else if step % 5 == 0 {
                    '*'
                } else {
                    '-'
                };
                let low = previous_y.min(y);
                let high = previous_y.max(y);
                for bridge_y in low..=high {
                    frame.put(
                        x,
                        bridge_y,
                        pathology_cell(
                            intensity * fade,
                            activity,
                            options.mode,
                            if bridge_y == y { glyph } else { '|' },
                        ),
                    );
                }
                previous_y = y;
            }
        }
        TcpVisual::ResetSent => {
            let radius = (3.0 + weight * 4.0).round() as i32;
            let x = center_x.round() as i32;
            let y = center_y.round() as i32;
            for offset in -radius..=radius {
                let fade = 1.0 - offset.unsigned_abs() as f32 / (radius + 1) as f32;
                frame.put(
                    x + offset,
                    y,
                    pathology_cell(
                        intensity * fade,
                        activity,
                        options.mode,
                        if offset == 0 { 'X' } else { '-' },
                    ),
                );
            }
            for offset in 1..=radius / 2 + 1 {
                frame.put(
                    x + offset,
                    y - offset,
                    pathology_cell(intensity * 0.72, activity, options.mode, '/'),
                );
                frame.put(
                    x + offset,
                    y + offset,
                    pathology_cell(intensity * 0.72, activity, options.mode, '\\'),
                );
            }
        }
        TcpVisual::ResetReceived => {
            let radius = (3.0 + weight * 4.0).round() as i32;
            let x = center_x.round() as i32;
            let y = center_y.round() as i32;
            for offset in -radius..=radius {
                let fade = 1.0 - offset.unsigned_abs() as f32 / (radius + 1) as f32;
                frame.put(
                    x,
                    y + offset,
                    pathology_cell(
                        intensity * fade,
                        activity,
                        options.mode,
                        if offset == 0 { '!' } else { '|' },
                    ),
                );
            }
            for offset in 1..=radius {
                frame.put(
                    x - offset,
                    y,
                    pathology_cell(intensity * 0.62, activity, options.mode, '<'),
                );
            }
        }
    }
}

fn pathology_cell(
    intensity: f32,
    activity: &Activity,
    mode: DisplayMode,
    ascii_glyph: char,
) -> Cell {
    let glyph = match mode {
        DisplayMode::Ascii => ascii_glyph,
        DisplayMode::Ansi => match ascii_glyph {
            '/' => '╱',
            '\\' => '╲',
            '-' => '━',
            '|' => '┃',
            'X' => '✕',
            '!' => '◆',
            '<' => '◀',
            other => other,
        },
    };
    Cell {
        glyph,
        intensity: intensity.clamp(0.0, 1.0),
        category: activity.category,
        direction: activity.direction,
    }
}

fn waterfall(
    snapshot: &ModelSnapshot,
    frame: &mut RenderFrame,
    field_height: u16,
    options: &ViewOptions,
) {
    let history = snapshot.decay_seconds * 2.5;
    for activity in &snapshot.activity {
        let age = (snapshot.now - activity.born).max(0.0);
        if age > history {
            continue;
        }
        if activity.category == stable_hash(crate::flight_recorder::TRIGGER_CATEGORY.as_bytes()) {
            let x = (f64::from(frame.width.saturating_sub(1))
                * (1.0 - age / history).clamp(0.0, 1.0))
            .round() as i32;
            for y in 0..field_height {
                frame.put(
                    x,
                    i32::from(y),
                    marker_cell((1.0 - age / history) as f32, options.mode),
                );
            }
            continue;
        }
        let category = activity.category;
        let flow_band = (activity.flow % u64::from(field_height)) as u16;
        let category_band = (category % u64::from(field_height)) as u16;
        let y = (category_band * 2 + flow_band) / 3;
        let x = (f64::from(frame.width.saturating_sub(1)) * (1.0 - age / history).clamp(0.0, 1.0))
            .round() as i32;
        let weight = ((activity.magnitude + 1.0).log2() / 11.0).clamp(0.08, 1.0);
        let intensity =
            (weight * f64::from(options.gain) * (1.0 - age / history)).clamp(0.03, 1.0) as f32;
        frame.put(
            x,
            i32::from(y),
            visual_cell(intensity, category, activity.direction, options.mode),
        );
        if let Some(pathology) = tcp_visual(category) {
            match pathology {
                TcpVisual::Retransmit => {
                    let offset = (activity.flow % 3) as i32 - 1;
                    frame.put(
                        x.saturating_sub(1),
                        i32::from(y) + offset,
                        pathology_cell(intensity * 0.8, activity, options.mode, '/'),
                    );
                    frame.put(
                        x.saturating_add(1),
                        i32::from(y) - offset,
                        pathology_cell(intensity * 0.8, activity, options.mode, '\\'),
                    );
                }
                TcpVisual::ResetSent => {
                    for offset in -2..=2 {
                        frame.put(
                            x + offset,
                            i32::from(y),
                            pathology_cell(
                                intensity,
                                activity,
                                options.mode,
                                if offset == 0 { 'X' } else { '-' },
                            ),
                        );
                    }
                }
                TcpVisual::ResetReceived => {
                    for offset in -2..=2 {
                        frame.put(
                            x,
                            i32::from(y) + offset,
                            pathology_cell(
                                intensity,
                                activity,
                                options.mode,
                                if offset == 0 { '!' } else { '|' },
                            ),
                        );
                    }
                }
            }
        }
        if activity.magnitude > 900.0 {
            frame.put(
                x,
                i32::from(y).saturating_sub(1),
                visual_cell(intensity * 0.65, category, activity.direction, options.mode),
            );
        }
    }
}

/// Prototype lane contract for the meter view. The temporal model deliberately
/// retains category hashes, not strings, so interpretation (label, gauge vs
/// rate, full-scale ceiling) lives here with the consumer, keyed by stable
/// hash — the same move as `tcp_visual`. Gauges carry a 0..1 ratio in event
/// magnitude; rate lanes carry per-sample byte deltas and are normalized by a
/// fixed prototype ceiling (calibrated-ish; discovery/config comes later).
struct MeterLane {
    category: &'static str,
    label: &'static str,
    gauge: bool,
    ceiling: f64,
    direction: Direction,
}

const METER_LANES: &[MeterLane] = &[
    MeterLane {
        category: "host.cpu",
        label: "cpu",
        gauge: true,
        ceiling: 1.0,
        direction: Direction::Neutral,
    },
    MeterLane {
        category: "host.memory",
        label: "mem",
        gauge: true,
        ceiling: 1.0,
        direction: Direction::Neutral,
    },
    MeterLane {
        category: "host.net.rx",
        label: "net rx",
        gauge: false,
        ceiling: 125_000_000.0,
        direction: Direction::Inbound,
    },
    MeterLane {
        category: "host.net.tx",
        label: "net tx",
        gauge: false,
        ceiling: 125_000_000.0,
        direction: Direction::Outbound,
    },
    MeterLane {
        category: "proc.io.read",
        label: "dsk r",
        gauge: false,
        ceiling: 512.0 * 1024.0 * 1024.0,
        direction: Direction::Inbound,
    },
    MeterLane {
        category: "proc.io.write",
        label: "dsk w",
        gauge: false,
        ceiling: 512.0 * 1024.0 * 1024.0,
        direction: Direction::Outbound,
    },
];

const METER_GAUGE_HOLD_SECONDS: f64 = 0.6;
const METER_GAUGE_RELEASE_SECONDS: f64 = 1.2;

struct MeterBar {
    label: String,
    level: f64,
    peak: f64,
    category: u64,
    direction: Direction,
}

fn meter(
    snapshot: &ModelSnapshot,
    frame: &mut RenderFrame,
    field_height: u16,
    options: &ViewOptions,
) {
    let retention = snapshot.decay_seconds * 2.5;
    let interval = options
        .proc
        .map(|proc| proc.interval_ms as f64 / 1_000.0)
        .filter(|seconds| *seconds > 0.0)
        .unwrap_or(0.25);
    let window = (interval * 2.5).clamp(1.0_f64.min(retention), retention.max(0.25));
    let hashes: Vec<u64> = METER_LANES
        .iter()
        .map(|lane| stable_hash(lane.category.as_bytes()))
        .collect();
    let hold = (interval * 2.0).max(METER_GAUGE_HOLD_SECONDS);
    let mut levels = vec![0.0_f64; METER_LANES.len()];
    let mut rate_sums = vec![0.0_f64; METER_LANES.len()];
    let mut peaks = vec![0.0_f64; METER_LANES.len()];
    let mut matched = false;
    for activity in &snapshot.activity {
        let age = (snapshot.now - activity.born).max(0.0);
        if age > retention {
            continue;
        }
        let Some(index) = hashes.iter().position(|hash| *hash == activity.category) else {
            continue;
        };
        matched = true;
        let lane = &METER_LANES[index];
        if lane.gauge {
            // Instant attack: the newest sample holds at full value; older
            // samples release linearly, all derived from timestamped snapshot
            // history — no hidden view state.
            let value = (activity.magnitude / lane.ceiling).clamp(0.0, 1.0);
            let envelope = if age <= hold {
                1.0
            } else {
                (1.0 - (age - hold) / METER_GAUGE_RELEASE_SECONDS).max(0.0)
            };
            levels[index] = levels[index].max(value * envelope);
            peaks[index] = peaks[index].max(value);
        } else {
            if age <= window {
                rate_sums[index] += activity.magnitude;
            }
            // Approximate instantaneous rate for the retention-bounded peak.
            peaks[index] = peaks[index].max((activity.magnitude / interval) / lane.ceiling);
        }
    }
    if !matched {
        meter_generic(snapshot, frame, field_height, options);
        return;
    }
    let bars: Vec<MeterBar> = METER_LANES
        .iter()
        .enumerate()
        .map(|(index, lane)| MeterBar {
            label: lane.label.to_owned(),
            level: if lane.gauge {
                levels[index]
            } else {
                (rate_sums[index] / window / lane.ceiling).clamp(0.0, 1.0)
            },
            peak: peaks[index].clamp(0.0, 1.0),
            category: hashes[index],
            direction: lane.direction,
        })
        .collect();
    draw_bars(frame, field_height, &bars, options.mode);
}

/// Fallback for sources with no known meter lanes: relative "loudness" bars
/// bucketed by category hash, normalized to the loudest bucket. Explicitly a
/// music-visualizer scale, not a calibrated one.
fn meter_generic(
    snapshot: &ModelSnapshot,
    frame: &mut RenderFrame,
    field_height: u16,
    options: &ViewOptions,
) {
    let retention = snapshot.decay_seconds * 2.5;
    let window = 1.0_f64.min(retention.max(0.25));
    let buckets = usize::from((frame.width / 8).clamp(4, 16));
    let mut sums = vec![0.0_f64; buckets];
    let mut categories = vec![0_u64; buckets];
    let mut directions = vec![Direction::Unknown; buckets];
    for activity in &snapshot.activity {
        let age = (snapshot.now - activity.born).max(0.0);
        if age > window {
            continue;
        }
        let index = (activity.category % buckets as u64) as usize;
        sums[index] += activity.magnitude;
        categories[index] = activity.category;
        directions[index] = activity.direction;
    }
    let loudest = sums.iter().cloned().fold(0.0_f64, f64::max);
    if loudest <= 0.0 {
        return;
    }
    let bars: Vec<MeterBar> = (0..buckets)
        .map(|index| MeterBar {
            label: format!("{index:x}"),
            level: ((sums[index] / loudest) * f64::from(options.gain)).clamp(0.0, 1.0),
            peak: 0.0,
            category: if categories[index] == 0 {
                index as u64
            } else {
                categories[index]
            },
            direction: directions[index],
        })
        .collect();
    draw_bars(frame, field_height, &bars, options.mode);
}

fn draw_bars(frame: &mut RenderFrame, field_height: u16, bars: &[MeterBar], mode: DisplayMode) {
    if bars.is_empty() || frame.width == 0 || field_height == 0 {
        return;
    }
    let label_row = field_height >= 3;
    let bar_rows = if label_row {
        field_height - 1
    } else {
        field_height
    };
    let columns = bars.len() as u16;
    let column_width = (frame.width / columns).max(1);
    let start_x = (frame.width.saturating_sub(column_width * columns)) / 2;
    for (index, bar) in bars.iter().enumerate() {
        let column_x = i32::from(start_x) + i32::from(column_width) * index as i32;
        let bar_width = i32::from(column_width.saturating_sub(2).clamp(1, 8));
        let bar_x = column_x + (i32::from(column_width) - bar_width) / 2;
        let level_rows = (bar.level.clamp(0.0, 1.0) * f64::from(bar_rows)).round() as i32;
        for row in 0..level_rows {
            let y = i32::from(bar_rows) - 1 - row;
            let intensity = 0.3 + 0.65 * (row + 1) as f32 / f32::from(bar_rows);
            for x in bar_x..bar_x + bar_width {
                frame.put(
                    x,
                    y,
                    visual_cell(intensity, bar.category, bar.direction, mode),
                );
            }
        }
        if level_rows == 0 {
            for x in bar_x..bar_x + bar_width {
                frame.put(
                    x,
                    i32::from(bar_rows) - 1,
                    Cell {
                        glyph: match mode {
                            DisplayMode::Ascii => '.',
                            DisplayMode::Ansi => '·',
                        },
                        intensity: 0.15,
                        category: bar.category,
                        direction: bar.direction,
                    },
                );
            }
        }
        let peak_rows = (bar.peak.clamp(0.0, 1.0) * f64::from(bar_rows)).round() as i32;
        if peak_rows > level_rows && peak_rows >= 1 {
            let y = i32::from(bar_rows) - peak_rows;
            for x in bar_x..bar_x + bar_width {
                frame.put(
                    x,
                    y,
                    Cell {
                        glyph: match mode {
                            DisplayMode::Ascii => '-',
                            DisplayMode::Ansi => '─',
                        },
                        intensity: 0.85,
                        category: bar.category,
                        direction: bar.direction,
                    },
                );
            }
        }
        if label_row {
            let label: String = bar.label.chars().take(usize::from(column_width)).collect();
            let label_x = column_x
                + (i32::from(column_width) - i32::try_from(label.chars().count()).unwrap_or(0)) / 2;
            for (offset, glyph) in label.chars().enumerate() {
                frame.put(
                    label_x + offset as i32,
                    i32::from(field_height) - 1,
                    Cell {
                        glyph,
                        intensity: 0.45,
                        category: bar.category,
                        direction: Direction::Neutral,
                    },
                );
            }
        }
    }
}

fn trigger_marker(frame: &mut RenderFrame, field_height: u16, intensity: f32, mode: DisplayMode) {
    let x = i32::from(frame.width / 2);
    for y in 0..field_height {
        frame.put(x, i32::from(y), marker_cell(intensity, mode));
    }
}

fn marker_cell(intensity: f32, mode: DisplayMode) -> Cell {
    Cell {
        glyph: match mode {
            DisplayMode::Ascii => '|',
            DisplayMode::Ansi => '│',
        },
        intensity: intensity.clamp(0.25, 1.0),
        category: stable_hash(crate::flight_recorder::TRIGGER_CATEGORY.as_bytes()),
        direction: Direction::Neutral,
    }
}

fn visual_cell(intensity: f32, category: u64, direction: Direction, mode: DisplayMode) -> Cell {
    let clamped = intensity.clamp(0.0, 1.0);
    let glyph = match mode {
        DisplayMode::Ascii => {
            let index = (clamped * (ASCII_DENSITY.len() - 1) as f32).round() as usize;
            let density = ASCII_DENSITY[index] as char;
            if clamped > 0.72 {
                match category % 5 {
                    0 => '*',
                    1 => '#',
                    2 => '@',
                    3 => '+',
                    _ => density,
                }
            } else {
                density
            }
        }
        DisplayMode::Ansi => {
            let index = (clamped * (ANSI_DENSITY.len() - 1) as f32).round() as usize;
            if clamped > 0.78 && category % 4 == 0 {
                '◆'
            } else {
                ANSI_DENSITY[index]
            }
        }
    };
    Cell {
        glyph,
        intensity: clamped,
        category,
        direction,
    }
}

fn compact_magnitude(value: f64) -> String {
    if value.abs() < 0.5 {
        "0".to_owned()
    } else if value >= 1_000_000.0 {
        format!("{:.1}m", value / 1_000_000.0)
    } else if value >= 1_000.0 {
        format!("{:.1}k", value / 1_000.0)
    } else {
        format!("{value:.0}")
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        cli::ViewKind, event::NormalizedEvent, model::TemporalModel, source::demo::DemoSource,
    };

    use super::*;

    fn snapshot() -> ModelSnapshot {
        let mut model = TemporalModel::default();
        for (index, event) in DemoSource::new(42).take(180).enumerate() {
            model.ingest(event, index as f64 * 0.035);
        }
        model.snapshot()
    }

    fn options(view: ViewKind, mode: DisplayMode) -> ViewOptions {
        ViewOptions {
            mode,
            view,
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
        }
    }

    #[test]
    fn ascii_snapshot_is_strict_plain_ascii_without_escapes() {
        let frame = compose(
            &snapshot(),
            100,
            30,
            &options(ViewKind::Weather, DisplayMode::Ascii),
        );
        let text = frame.plain_text();
        assert!(text.is_ascii());
        assert!(!text.contains('\x1b'));
        assert!(
            text.bytes()
                .all(|byte| byte == b'\n' || (b' '..=b'~').contains(&byte))
        );
    }

    #[test]
    fn weather_and_waterfall_are_observably_distinct() {
        let snapshot = snapshot();
        let weather = compose(
            &snapshot,
            80,
            24,
            &options(ViewKind::Weather, DisplayMode::Ascii),
        );
        let waterfall = compose(
            &snapshot,
            80,
            24,
            &options(ViewKind::Waterfall, DisplayMode::Ascii),
        );
        let differing = weather
            .cells
            .iter()
            .zip(&waterfall.cells)
            .filter(|(left, right)| left.glyph != right.glyph)
            .count();
        assert!(differing > 100);
    }

    #[test]
    fn terminal_size_edges_do_not_panic() {
        for (width, height) in [(1, 1), (2, 1), (1, 2), (10, 3)] {
            let frame = compose(
                &snapshot(),
                width,
                height,
                &options(ViewKind::Weather, DisplayMode::Ascii),
            );
            assert_eq!(frame.cells.len(), usize::from(width) * usize::from(height));
        }
    }

    #[test]
    fn magnitude_and_direction_change_weather_output() {
        let mut low_model = TemporalModel::default();
        let mut low = DemoSource::new(2).next().unwrap();
        low.magnitude = 2.0;
        low.direction = Direction::Inbound;
        low_model.ingest(low.clone(), 0.0);
        low_model.advance(0.5);

        let mut high_model = TemporalModel::default();
        low.magnitude = 8_000.0;
        low.direction = Direction::Outbound;
        high_model.ingest(low, 0.0);
        high_model.advance(0.5);

        let low_frame = compose(
            &low_model.snapshot(),
            80,
            24,
            &options(ViewKind::Weather, DisplayMode::Ascii),
        );
        let high_frame = compose(
            &high_model.snapshot(),
            80,
            24,
            &options(ViewKind::Weather, DisplayMode::Ascii),
        );
        assert_ne!(low_frame.plain_text(), high_frame.plain_text());
    }

    fn scheduler_snapshot() -> ModelSnapshot {
        let mut model = TemporalModel::default();
        for line in include_str!("../examples/scheduler.ndjson").lines() {
            let event: NormalizedEvent = serde_json::from_str(line).unwrap();
            let now = event.timestamp.unwrap();
            model.ingest(event, now);
        }
        model.snapshot()
    }

    #[test]
    fn scheduler_fixture_snapshot_is_deterministic_and_has_clean_status() {
        let render = || {
            compose(
                &scheduler_snapshot(),
                80,
                24,
                &options(ViewKind::Weather, DisplayMode::Ascii),
            )
            .plain_text()
        };
        let first = render();
        assert_eq!(first, render());
        let status = first.lines().last().unwrap();
        assert!(status.contains("sched/s"));
        assert!(status.contains("loss 0/0/0"));
        assert!(!status.contains('@'));
    }

    #[test]
    fn scheduler_fixture_is_visibly_distinct_from_generic_lines() {
        let scheduler = compose(
            &scheduler_snapshot(),
            80,
            24,
            &options(ViewKind::Weather, DisplayMode::Ascii),
        );
        let generic = compose(
            &snapshot(),
            80,
            24,
            &options(ViewKind::Weather, DisplayMode::Ascii),
        );
        let differing = scheduler
            .cells
            .iter()
            .zip(&generic.cells)
            .filter(|(left, right)| left.glyph != right.glyph)
            .count();
        assert!(differing > 150);
    }

    fn tcp_fixture_snapshot() -> ModelSnapshot {
        let mut model = TemporalModel::default();
        for line in include_str!("../tests/fixtures/tcp-pathology.ndjson").lines() {
            let event: NormalizedEvent = serde_json::from_str(line).unwrap();
            let now = event.timestamp.unwrap();
            model.ingest(event, now);
        }
        model.snapshot()
    }

    fn proc_fixture_snapshot() -> ModelSnapshot {
        let mut model = TemporalModel::default();
        for line in include_str!("../tests/fixtures/proc-activity.ndjson").lines() {
            let event: NormalizedEvent = serde_json::from_str(line).unwrap();
            model.ingest(event.clone(), event.timestamp.unwrap());
        }
        model.snapshot()
    }

    #[test]
    fn proc_fixture_snapshot_is_deterministic_with_compact_source_status() {
        let mut proc_options = options(ViewKind::Weather, DisplayMode::Ascii);
        proc_options.proc = Some(ProcSourceStatus {
            interval_ms: 250,
            tracked: 4,
            unreadable: 2,
            vanished: 1,
            source_dropped: 3,
        });
        let render = || compose(&proc_fixture_snapshot(), 100, 30, &proc_options).plain_text();
        let first = render();
        assert_eq!(first, render());
        let status = first.lines().last().unwrap();
        assert!(status.contains("proc/s"));
        assert!(status.contains("250ms"));
        assert!(status.contains("miss 2/1 drop 3"));
        assert!(!status.contains('@'));
    }

    #[test]
    fn proc_cpu_and_io_have_distinct_directional_weather() {
        let base: NormalizedEvent = serde_json::from_str(
            include_str!("../tests/fixtures/proc-activity.ndjson")
                .lines()
                .nth(2)
                .unwrap(),
        )
        .unwrap();
        let render = |category: &str, direction: Direction| {
            let mut event = base.clone();
            event.category = category.to_owned();
            event.direction = direction;
            let mut model = TemporalModel::default();
            model.ingest(event, 0.0);
            let mut render_options = options(ViewKind::Weather, DisplayMode::Ascii);
            render_options.particles = false;
            compose(&model.snapshot(), 80, 24, &render_options).plain_text()
        };
        let cpu = render("proc.cpu", Direction::Neutral);
        let read = render("proc.io.read", Direction::Inbound);
        let write = render("proc.io.write", Direction::Outbound);
        assert_ne!(cpu, read);
        assert_ne!(read, write);
    }

    #[test]
    fn tcp_fixture_snapshot_is_deterministic_with_clean_loss_status() {
        let mut tcp_options = options(ViewKind::Weather, DisplayMode::Ascii);
        tcp_options.kernel_dropped = 2;
        tcp_options.collector_dropped = 3;
        tcp_options.ipc_dropped = 4;
        tcp_options.dropped = 5;
        let render = || compose(&tcp_fixture_snapshot(), 100, 30, &tcp_options).plain_text();
        let first = render();
        assert_eq!(first, render());
        let status = first.lines().last().unwrap();
        assert!(status.contains("tcp/s"));
        assert!(status.contains("loss 2/3/4/5"));
        assert!(!status.contains('@'));
    }

    #[test]
    fn retransmit_and_resets_have_distinct_ascii_impact_grammar() {
        let base: NormalizedEvent = serde_json::from_str(
            include_str!("../tests/fixtures/tcp-pathology.ndjson")
                .lines()
                .next()
                .unwrap(),
        )
        .unwrap();
        let render = |category: &str, direction: Direction| {
            let mut event = base.clone();
            event.category = category.to_owned();
            event.direction = direction;
            let mut model = TemporalModel::default();
            model.ingest(event, 0.0);
            compose(
                &model.snapshot(),
                80,
                24,
                &options(ViewKind::Weather, DisplayMode::Ascii),
            )
            .plain_text()
        };
        let retransmit = render("tcp.retransmit", Direction::Outbound);
        let sent = render("tcp.reset.send", Direction::Outbound);
        let received = render("tcp.reset.receive", Direction::Inbound);
        assert!(retransmit.contains('/') || retransmit.contains('\\'));
        assert!(sent.contains('X'));
        assert!(received.contains('!'));
        assert_ne!(retransmit, sent);
        assert_ne!(sent, received);
    }

    #[test]
    fn retransmit_strike_occupies_its_stable_flow_lane() {
        let event: NormalizedEvent = serde_json::from_str(
            include_str!("../tests/fixtures/tcp-pathology.ndjson")
                .lines()
                .nth(2)
                .unwrap(),
        )
        .unwrap();
        let mut model = TemporalModel::default();
        model.ingest(event, 0.0);
        let snapshot = model.snapshot();
        let expected_y = (snapshot.activity[0].lane % 23) as i32;
        let frame = compose(
            &snapshot,
            80,
            24,
            &options(ViewKind::Weather, DisplayMode::Ascii),
        );
        assert!(frame.cells.iter().enumerate().any(|(index, cell)| {
            let y = (index / usize::from(frame.width)) as i32;
            cell.intensity > 0.7 && (y - expected_y).abs() <= 3
        }));
    }

    #[test]
    fn tcp_burst_remains_bounded_and_settle_clears_the_field() {
        let event: NormalizedEvent = serde_json::from_str(
            include_str!("../tests/fixtures/tcp-pathology.ndjson")
                .lines()
                .nth(7)
                .unwrap(),
        )
        .unwrap();
        let mut model = TemporalModel::new(0.5);
        for _ in 0..5_000 {
            model.ingest(event.clone(), 0.0);
        }
        assert_eq!(model.snapshot().activity.len(), 4_096);
        model.advance(2.0);
        assert!(model.snapshot().activity.is_empty());
    }

    #[test]
    fn flight_status_is_restrained_and_precedes_source_metrics() {
        let mut flight_options = options(ViewKind::Weather, DisplayMode::Ascii);
        flight_options.flight = Some(FlightStatus {
            state: FlightState::Armed,
            retained_events: 40,
            retained_bytes: 8_000,
            retained_duration: 8.4,
            pre_trigger_evictions: 12,
            tail_elapsed: 0.0,
            tail_duration: 5.0,
            trigger_kind: None,
            output: std::path::PathBuf::from("incident.ndjson"),
        });
        let rendered = compose(&tcp_fixture_snapshot(), 100, 30, &flight_options).plain_text();
        let status = rendered.lines().last().unwrap();
        assert!(status.starts_with(" rec armed pre 8.4s evict 12 |"));
        assert!(status.contains("tcp/s"));
        assert!(!status.contains('@'));
        assert!(!status.contains("-0.0"));
    }

    #[test]
    fn replay_trigger_marker_is_visible_and_phase_is_named() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/tcp-flight-incident.ndjson");
        let mut replay = crate::recording::ReplaySource::open(&path, 1.0).unwrap();
        let mut model = crate::model::TemporalModel::default();
        let mut now = 0.0;
        while let Some((delay, event)) = replay.next_timed().unwrap() {
            now += delay.as_secs_f64();
            model.ingest(event, now);
        }
        let rendered = compose(
            &model.snapshot(),
            100,
            30,
            &options(ViewKind::Waterfall, DisplayMode::Ascii),
        )
        .plain_text();
        assert!(rendered.lines().last().unwrap().contains("replay POST tcp"));
        let rows: Vec<_> = rendered.lines().take(29).collect();
        assert!((0..100).any(|column| {
            rows.iter()
                .filter(|row| row.chars().nth(column) == Some('|'))
                .count()
                > 10
        }));
    }

    #[test]
    fn particle_layer_is_deterministic_and_can_be_disabled_without_touching_the_field() {
        let snapshot = tcp_fixture_snapshot();
        let with_particles = compose(
            &snapshot,
            100,
            30,
            &options(ViewKind::Weather, DisplayMode::Ascii),
        );
        let mut disabled = options(ViewKind::Weather, DisplayMode::Ascii);
        disabled.particles = false;
        let without_particles = compose(&snapshot, 100, 30, &disabled);
        assert_ne!(with_particles, without_particles);
        assert_eq!(
            with_particles,
            compose(
                &snapshot,
                100,
                30,
                &options(ViewKind::Weather, DisplayMode::Ascii)
            )
        );

        let mut no_particle_snapshot = snapshot.clone();
        no_particle_snapshot.particles.clear();
        assert_eq!(
            without_particles,
            compose(&no_particle_snapshot, 100, 30, &disabled)
        );
    }

    #[test]
    fn ascii_and_ansi_particles_use_materially_different_glyphs() {
        let mut snapshot = tcp_fixture_snapshot();
        snapshot.activity.clear();
        let ascii = compose(
            &snapshot,
            100,
            30,
            &options(ViewKind::Weather, DisplayMode::Ascii),
        );
        let ansi = compose(
            &snapshot,
            100,
            30,
            &options(ViewKind::Weather, DisplayMode::Ansi),
        );
        let differing = ascii
            .cells
            .iter()
            .zip(&ansi.cells)
            .filter(|(left, right)| left.glyph != right.glyph)
            .count();
        assert!(differing >= 8);
        assert!(ascii.plain_text().is_ascii());
        assert!(!ascii.plain_text().contains('\x1b'));
    }

    fn meter_event(category: &str, magnitude: f64, direction: Direction) -> NormalizedEvent {
        NormalizedEvent {
            v: 1,
            timestamp: None,
            category: category.to_owned(),
            origin: Some("host".to_owned()),
            target: None,
            magnitude,
            direction,
            labels: std::collections::BTreeMap::from([(
                "synesthesia.lane".to_owned(),
                category.to_owned(),
            )]),
        }
    }

    #[test]
    fn meter_lanes_render_labeled_calibrated_ascii_bars_deterministically() {
        let mut model = TemporalModel::default();
        model.ingest(meter_event("host.cpu", 0.8, Direction::Neutral), 0.1);
        model.ingest(meter_event("host.memory", 0.5, Direction::Neutral), 0.1);
        model.ingest(
            meter_event("host.net.rx", 20_000_000.0, Direction::Inbound),
            0.1,
        );
        model.advance(0.2);
        let snapshot = model.snapshot();
        let render = || {
            compose(
                &snapshot,
                100,
                30,
                &options(ViewKind::Meter, DisplayMode::Ascii),
            )
            .plain_text()
        };
        let first = render();
        assert_eq!(first, render());
        assert!(first.is_ascii());
        assert!(!first.contains('\x1b'));
        for label in ["cpu", "mem", "net rx", "net tx", "dsk r", "dsk w"] {
            assert!(first.contains(label), "missing lane label {label}");
        }

        let mut busier = TemporalModel::default();
        busier.ingest(meter_event("host.cpu", 0.2, Direction::Neutral), 0.1);
        busier.ingest(meter_event("host.memory", 0.5, Direction::Neutral), 0.1);
        busier.ingest(
            meter_event("host.net.rx", 20_000_000.0, Direction::Inbound),
            0.1,
        );
        busier.advance(0.2);
        let lower = compose(
            &busier.snapshot(),
            100,
            30,
            &options(ViewKind::Meter, DisplayMode::Ascii),
        )
        .plain_text();
        assert_ne!(first, lower);
    }

    #[test]
    fn meter_peak_cap_outlives_a_dropped_level_within_retention() {
        let mut spiked = TemporalModel::default();
        spiked.ingest(meter_event("host.cpu", 0.9, Direction::Neutral), 0.0);
        spiked.ingest(meter_event("host.cpu", 0.1, Direction::Neutral), 2.0);
        spiked.advance(2.5);

        let mut quiet = TemporalModel::default();
        quiet.ingest(meter_event("host.cpu", 0.1, Direction::Neutral), 2.0);
        quiet.advance(2.5);

        let mut view_options = options(ViewKind::Meter, DisplayMode::Ascii);
        view_options.particles = false;
        let with_peak = compose(&spiked.snapshot(), 100, 30, &view_options).plain_text();
        let without_peak = compose(&quiet.snapshot(), 100, 30, &view_options).plain_text();
        assert_ne!(with_peak, without_peak);
        // The spike sample is old enough that its level released, so the
        // difference above the resting bar can only be the peak cap marker.
        let upper_with: String = with_peak.lines().take(10).collect();
        let upper_without: String = without_peak.lines().take(10).collect();
        assert!(upper_with.contains('-'));
        assert!(!upper_without.contains('-'));
    }

    #[test]
    fn meter_falls_back_to_relative_bars_for_unknown_categories() {
        let snapshot = snapshot();
        let render = || {
            compose(
                &snapshot,
                100,
                30,
                &options(ViewKind::Meter, DisplayMode::Ascii),
            )
            .plain_text()
        };
        let first = render();
        assert_eq!(first, render());
        assert!(first.is_ascii());
        let field: String = first.lines().take(29).collect();
        assert!(field.chars().any(|glyph| glyph != ' '));
        let weather = compose(
            &snapshot,
            100,
            30,
            &options(ViewKind::Weather, DisplayMode::Ascii),
        )
        .plain_text();
        assert_ne!(first, weather);
    }

    #[test]
    fn meter_terminal_size_edges_do_not_panic() {
        let mut model = TemporalModel::default();
        model.ingest(meter_event("host.cpu", 0.8, Direction::Neutral), 0.1);
        model.advance(0.2);
        let snapshot = model.snapshot();
        for (width, height) in [(1, 1), (2, 1), (1, 2), (10, 3), (6, 2), (100, 30)] {
            let frame = compose(
                &snapshot,
                width,
                height,
                &options(ViewKind::Meter, DisplayMode::Ascii),
            );
            assert_eq!(frame.cells.len(), usize::from(width) * usize::from(height));
        }
    }

    #[test]
    fn composing_particles_does_not_mutate_model_state() {
        let model_snapshot = snapshot();
        let before = model_snapshot.clone();
        let _ = compose(
            &model_snapshot,
            80,
            24,
            &options(ViewKind::Waterfall, DisplayMode::Ascii),
        );
        assert_eq!(model_snapshot, before);
    }
}
