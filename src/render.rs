use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    widgets::Widget,
};

use crate::{
    cli::{DisplayMode, Theme},
    event::Direction,
};

#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    pub glyph: char,
    pub intensity: f32,
    pub category: u64,
    pub direction: Direction,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            glyph: ' ',
            intensity: 0.0,
            category: 0,
            direction: Direction::Unknown,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RenderFrame {
    pub width: u16,
    pub height: u16,
    pub cells: Vec<Cell>,
}

impl RenderFrame {
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            width,
            height,
            cells: vec![Cell::default(); usize::from(width) * usize::from(height)],
        }
    }

    pub fn put(&mut self, x: i32, y: i32, cell: Cell) {
        if x < 0 || y < 0 || x >= i32::from(self.width) || y >= i32::from(self.height) {
            return;
        }
        let index = y as usize * usize::from(self.width) + x as usize;
        if cell.intensity >= self.cells[index].intensity {
            self.cells[index] = cell;
        }
    }

    pub fn put_overlay(&mut self, x: i32, y: i32, cell: Cell) {
        if x < 0 || y < 0 || x >= i32::from(self.width) || y >= i32::from(self.height) {
            return;
        }
        let index = y as usize * usize::from(self.width) + x as usize;
        self.cells[index] = cell;
    }

    pub fn write_status(&mut self, status: &str) {
        if self.height == 0 {
            return;
        }
        let y = self.height - 1;
        let row_start = usize::from(y) * usize::from(self.width);
        self.cells[row_start..row_start + usize::from(self.width)].fill(Cell::default());
        for (x, character) in status
            .chars()
            .filter(|character| character.is_ascii() && !character.is_ascii_control())
            .take(usize::from(self.width))
            .enumerate()
        {
            self.cells[row_start + x] = Cell {
                glyph: character,
                intensity: 0.45,
                category: 0,
                direction: Direction::Neutral,
            };
        }
    }

    pub fn plain_text(&self) -> String {
        let mut output =
            String::with_capacity((usize::from(self.width) + 1) * usize::from(self.height));
        for y in 0..self.height {
            let row_start = usize::from(y) * usize::from(self.width);
            let row = &self.cells[row_start..row_start + usize::from(self.width)];
            let last_visible = row
                .iter()
                .rposition(|cell| cell.glyph != ' ')
                .map_or(0, |index| index + 1);
            for cell in &row[..last_visible] {
                output.push(cell.glyph);
            }
            if y + 1 < self.height {
                output.push('\n');
            }
        }
        output
    }
}

/// How much color the terminal is believed to support. Colors degrade
/// truecolor -> 256-index approximation -> named 16-color, always computed
/// from the same theme RGB so every tier shows the same design.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ColorDepth {
    Truecolor,
    Xterm256,
    Ansi16,
}

pub struct GridWidget<'a> {
    pub frame: &'a RenderFrame,
    pub mode: DisplayMode,
    pub theme: Theme,
    pub depth: ColorDepth,
}

impl Widget for GridWidget<'_> {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        let width = self.frame.width.min(area.width);
        let height = self.frame.height.min(area.height);
        for y in 0..height {
            for x in 0..width {
                let index = usize::from(y) * usize::from(self.frame.width) + usize::from(x);
                let cell = &self.frame.cells[index];
                let style = if self.mode == DisplayMode::Ascii {
                    Style::default()
                } else {
                    Style::default().fg(color_for(
                        self.theme,
                        cell.intensity,
                        cell.category,
                        cell.direction,
                        self.depth,
                    ))
                };
                let symbol = cell.glyph.to_string();
                buffer[(area.x + x, area.y + y)]
                    .set_symbol(&symbol)
                    .set_style(style);
            }
        }
    }
}

fn color_for(
    theme: Theme,
    intensity: f32,
    category: u64,
    direction: Direction,
    depth: ColorDepth,
) -> Color {
    if depth == ColorDepth::Ansi16 {
        return ansi16_color(theme, intensity, category, direction);
    }
    let (red, green, blue) = theme_rgb(theme, intensity, category, direction);
    match depth {
        ColorDepth::Truecolor => Color::Rgb(red, green, blue),
        ColorDepth::Xterm256 => Color::Indexed(xterm256_index(red, green, blue)),
        ColorDepth::Ansi16 => unreachable!("handled above"),
    }
}

fn theme_rgb(theme: Theme, intensity: f32, category: u64, direction: Direction) -> (u8, u8, u8) {
    let level = (intensity.clamp(0.0, 1.0) * 255.0) as u8;
    match theme {
        Theme::Phosphor => (
            20 + level / 5,
            65 + level.saturating_mul(3) / 4,
            35 + ((category as u8) % 45),
        ),
        Theme::Amber => (85 + level.saturating_mul(2) / 3, 30 + level / 2, 8),
        Theme::Cold => match direction {
            // Saturating add carried over from the standalone overflow fix.
            Direction::Inbound => (30, 100 + level / 2, 130_u8.saturating_add(level / 2)),
            Direction::Outbound => (85 + level / 3, 45 + level / 3, 150 + level / 3),
            _ => (35 + level / 4, 90 + level / 2, 110 + level / 2),
        },
        Theme::Monochrome => (level, level, level),
        // Hue is derived from the stable category hash alone, so a lane keeps
        // its color regardless of how busy it is; intensity only brightens.
        Theme::Rainbow => hsv_to_rgb(
            category_hue(category),
            0.9,
            0.35 + 0.6 * intensity.clamp(0.0, 1.0),
        ),
        Theme::Pastel => hsv_to_rgb(
            category_hue(category),
            0.38,
            0.62 + 0.38 * intensity.clamp(0.0, 1.0),
        ),
    }
}

fn ansi16_color(theme: Theme, intensity: f32, category: u64, direction: Direction) -> Color {
    match theme {
        Theme::Phosphor => {
            if intensity > 0.68 {
                Color::LightGreen
            } else {
                Color::Green
            }
        }
        Theme::Amber => {
            if intensity > 0.68 {
                Color::LightYellow
            } else {
                Color::Yellow
            }
        }
        Theme::Cold => match direction {
            Direction::Inbound => Color::LightCyan,
            Direction::Outbound => Color::LightBlue,
            _ => Color::Cyan,
        },
        Theme::Monochrome => {
            if intensity > 0.68 {
                Color::White
            } else {
                Color::Gray
            }
        }
        Theme::Rainbow | Theme::Pastel => {
            let sextant = [
                (Color::Red, Color::LightRed),
                (Color::Yellow, Color::LightYellow),
                (Color::Green, Color::LightGreen),
                (Color::Cyan, Color::LightCyan),
                (Color::Blue, Color::LightBlue),
                (Color::Magenta, Color::LightMagenta),
            ][(category_hue(category) / 60.0) as usize % 6];
            // Pastel approximates as always-bright; rainbow brightens with
            // intensity like the other themes.
            if theme == Theme::Pastel || intensity > 0.68 {
                sextant.1
            } else {
                sextant.0
            }
        }
    }
}

/// Stable hue in degrees [0, 360) from a category hash.
fn category_hue(category: u64) -> f32 {
    (category % 360) as f32
}

fn hsv_to_rgb(hue: f32, saturation: f32, value: f32) -> (u8, u8, u8) {
    let hue = hue.rem_euclid(360.0);
    let chroma = value * saturation;
    let secondary = chroma * (1.0 - ((hue / 60.0) % 2.0 - 1.0).abs());
    let base = value - chroma;
    let (red, green, blue) = match hue {
        h if h < 60.0 => (chroma, secondary, 0.0),
        h if h < 120.0 => (secondary, chroma, 0.0),
        h if h < 180.0 => (0.0, chroma, secondary),
        h if h < 240.0 => (0.0, secondary, chroma),
        h if h < 300.0 => (secondary, 0.0, chroma),
        _ => (chroma, 0.0, secondary),
    };
    (
        ((red + base) * 255.0).round() as u8,
        ((green + base) * 255.0).round() as u8,
        ((blue + base) * 255.0).round() as u8,
    )
}

/// Quantize RGB into the xterm-256 palette: near-grays use the 24-step
/// grayscale ramp (232..=255); everything else the 6x6x6 color cube (16..=231).
fn xterm256_index(red: u8, green: u8, blue: u8) -> u8 {
    let spread = red.max(green).max(blue) - red.min(green).min(blue);
    if spread < 8 {
        let gray = (u16::from(red) + u16::from(green) + u16::from(blue)) / 3;
        return 232 + (gray * 24 / 256) as u8;
    }
    let quantize = |channel: u8| -> u8 { ((u16::from(channel) * 5 + 127) / 255) as u8 };
    16 + 36 * quantize(red) + 6 * quantize(green) + quantize(blue)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hue_of(rgb: (u8, u8, u8)) -> f32 {
        let (red, green, blue) = (
            f32::from(rgb.0) / 255.0,
            f32::from(rgb.1) / 255.0,
            f32::from(rgb.2) / 255.0,
        );
        let max = red.max(green).max(blue);
        let min = red.min(green).min(blue);
        let delta = max - min;
        if delta == 0.0 {
            return 0.0;
        }
        let hue = if max == red {
            60.0 * (((green - blue) / delta) % 6.0)
        } else if max == green {
            60.0 * ((blue - red) / delta + 2.0)
        } else {
            60.0 * ((red - green) / delta + 4.0)
        };
        hue.rem_euclid(360.0)
    }

    fn saturation_of(rgb: (u8, u8, u8)) -> f32 {
        let max = rgb.0.max(rgb.1).max(rgb.2);
        let min = rgb.0.min(rgb.1).min(rgb.2);
        if max == 0 {
            0.0
        } else {
            f32::from(max - min) / f32::from(max)
        }
    }

    #[test]
    fn rainbow_hue_is_stable_by_category_and_ignores_intensity() {
        let category = 12_345_u64;
        let dim = theme_rgb(Theme::Rainbow, 0.2, category, Direction::Neutral);
        let bright = theme_rgb(Theme::Rainbow, 0.95, category, Direction::Neutral);
        assert!((hue_of(dim) - hue_of(bright)).abs() < 4.0);
        assert!(bright.0.max(bright.1).max(bright.2) > dim.0.max(dim.1).max(dim.2));

        let other = theme_rgb(Theme::Rainbow, 0.2, category + 120, Direction::Neutral);
        assert!((hue_of(dim) - hue_of(other)).abs() > 30.0);
    }

    #[test]
    fn pastel_is_less_saturated_than_rainbow_at_equal_intensity() {
        for category in [7_u64, 90, 200, 311] {
            let rainbow = theme_rgb(Theme::Rainbow, 0.7, category, Direction::Neutral);
            let pastel = theme_rgb(Theme::Pastel, 0.7, category, Direction::Neutral);
            assert!(saturation_of(pastel) < saturation_of(rainbow));
            assert!((hue_of(pastel) - hue_of(rainbow)).abs() < 4.0);
        }
    }

    #[test]
    fn xterm256_tier_returns_valid_indexed_colors() {
        let color = color_for(
            Theme::Rainbow,
            0.8,
            77,
            Direction::Neutral,
            ColorDepth::Xterm256,
        );
        let Color::Indexed(index) = color else {
            panic!("expected indexed color, got {color:?}");
        };
        assert!((16..=231).contains(&index));

        let gray = color_for(
            Theme::Monochrome,
            0.5,
            77,
            Direction::Neutral,
            ColorDepth::Xterm256,
        );
        let Color::Indexed(index) = gray else {
            panic!("expected indexed gray, got {gray:?}");
        };
        assert!((232..=255).contains(&index));

        assert_eq!(xterm256_index(255, 255, 255), 255);
        assert_eq!(xterm256_index(0, 0, 0), 232);
        assert_eq!(xterm256_index(255, 0, 0), 196);
    }

    #[test]
    fn ansi16_tier_keeps_rainbow_lanes_distinct_and_named() {
        let colors: Vec<Color> = (0..6)
            .map(|sextant| {
                color_for(
                    Theme::Rainbow,
                    0.4,
                    sextant * 60,
                    Direction::Neutral,
                    ColorDepth::Ansi16,
                )
            })
            .collect();
        for (index, color) in colors.iter().enumerate() {
            assert!(!matches!(color, Color::Rgb(..) | Color::Indexed(_)));
            for other in &colors[index + 1..] {
                assert_ne!(color, other);
            }
        }
    }

    #[test]
    fn every_theme_produces_color_at_every_depth_without_panic() {
        for theme in [
            Theme::Phosphor,
            Theme::Amber,
            Theme::Cold,
            Theme::Monochrome,
            Theme::Rainbow,
            Theme::Pastel,
        ] {
            for depth in [
                ColorDepth::Truecolor,
                ColorDepth::Xterm256,
                ColorDepth::Ansi16,
            ] {
                for direction in [
                    Direction::Inbound,
                    Direction::Outbound,
                    Direction::Neutral,
                    Direction::Unknown,
                ] {
                    for intensity in [0.0_f32, 0.5, 1.0] {
                        let _ = color_for(theme, intensity, u64::MAX, direction, depth);
                    }
                }
            }
        }
    }

    #[test]
    fn cold_inbound_full_intensity_does_not_overflow() {
        assert_eq!(
            color_for(
                Theme::Cold,
                1.0,
                0,
                Direction::Inbound,
                ColorDepth::Truecolor
            ),
            Color::Rgb(30, 227, 255)
        );
    }

    #[test]
    fn status_row_overwrites_field_echoes() {
        let mut frame = RenderFrame::new(20, 3);
        frame.put(
            5,
            2,
            Cell {
                glyph: '@',
                intensity: 1.0,
                category: 9,
                direction: Direction::Outbound,
            },
        );
        frame.write_status(" status");
        let text = frame.plain_text();
        assert_eq!(text.lines().nth(2), Some(" status"));
    }
}
