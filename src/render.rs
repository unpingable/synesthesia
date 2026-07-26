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

pub struct GridWidget<'a> {
    pub frame: &'a RenderFrame,
    pub mode: DisplayMode,
    pub theme: Theme,
    pub truecolor: bool,
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
                        self.truecolor,
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
    truecolor: bool,
) -> Color {
    let level = (intensity.clamp(0.0, 1.0) * 255.0) as u8;
    if !truecolor {
        return match theme {
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
        };
    }
    match theme {
        Theme::Phosphor => Color::Rgb(
            20 + level / 5,
            65 + level.saturating_mul(3) / 4,
            35 + ((category as u8) % 45),
        ),
        Theme::Amber => Color::Rgb(85 + level.saturating_mul(2) / 3, 30 + level / 2, 8),
        Theme::Cold => match direction {
            Direction::Inbound => Color::Rgb(30, 100 + level / 2, 130 + level / 2),
            Direction::Outbound => Color::Rgb(85 + level / 3, 45 + level / 3, 150 + level / 3),
            _ => Color::Rgb(35 + level / 4, 90 + level / 2, 110 + level / 2),
        },
        Theme::Monochrome => Color::Rgb(level, level, level),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
