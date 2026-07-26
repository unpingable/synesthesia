use crate::{
    cli::{DisplayMode, ViewKind},
    event::Direction,
    model::ModelSnapshot,
    render::{Cell, RenderFrame},
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
    pub help: bool,
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
        }
    }
    let status = if options.help {
        " q/esc quit  space pause  1 weather  2 waterfall  a ascii/ansi  c theme  +/- gain  [] decay "
            .to_owned()
    } else {
        format!(
            " {:>5.1} evt/s  {:>7}/s  {:>3} flows  bad {} drop {}  {:?}/{:?}{}",
            snapshot.metrics.events_per_second,
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
    frame.write_status(&status);
    frame
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
        if activity.magnitude > 900.0 {
            frame.put(
                x,
                i32::from(y).saturating_sub(1),
                visual_cell(intensity * 0.65, category, activity.direction, options.mode),
            );
        }
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
    if value >= 1_000_000.0 {
        format!("{:.1}m", value / 1_000_000.0)
    } else if value >= 1_000.0 {
        format!("{:.1}k", value / 1_000.0)
    } else {
        format!("{value:.0}")
    }
}

#[cfg(test)]
mod tests {
    use crate::{cli::ViewKind, model::TemporalModel, source::demo::DemoSource};

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
}
