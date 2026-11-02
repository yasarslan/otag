//! Drawing a pane's screen.
//!
//! The daemon has already done the terminal emulation, so rendering is a
//! direct translation: rows of styled runs become cells in a ratatui buffer.

use otag_core::frame::{CellStyle, Color, ScreenState};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color as UiColor, Modifier, Style};
use ratatui::widgets::Widget;

/// A pane's screen, drawn as it was on the daemon's side.
///
/// Content that does not fit the area is cut off rather than wrapped: the
/// pane was drawn for a given size, and re-flowing it here would not match
/// what the program believes it drew.
#[derive(Debug, Clone, Copy)]
pub struct Pane<'a> {
    screen: &'a ScreenState,
}

impl<'a> Pane<'a> {
    /// Draw `screen`.
    #[must_use]
    pub const fn new(screen: &'a ScreenState) -> Self {
        Self { screen }
    }

    /// Where the terminal's own cursor belongs, if it should be shown.
    ///
    /// The cursor is left to the real terminal rather than drawn as a cell,
    /// so that it blinks and takes the shape the user has configured.
    #[must_use]
    pub fn cursor_position(&self, area: Rect) -> Option<Position> {
        let cursor = self.screen.cursor();
        if !cursor.visible {
            return None;
        }
        let position = Position {
            x: area.x.saturating_add(cursor.col),
            y: area.y.saturating_add(cursor.row),
        };
        area.contains(position).then_some(position)
    }
}

impl Widget for Pane<'_> {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        for (index, runs) in self.screen.rows().enumerate() {
            let Ok(row) = u16::try_from(index) else {
                break;
            };
            if row >= area.height {
                break;
            }
            let y = area.y.saturating_add(row);

            let mut x = area.x;
            for run in runs {
                if x >= area.right() {
                    break;
                }
                // A double-width character is written once and covers the
                // column after it, which the terminal paints as part of the
                // glyph; `set_stringn` advances past both.
                let (next, _) = buffer.set_stringn(
                    x,
                    y,
                    &run.text,
                    usize::from(area.right().saturating_sub(x)),
                    style_of(run.style),
                );

                if next == x {
                    // Nothing fitted; stop rather than loop forever.
                    break;
                }
                x = next;
            }
        }
    }
}

/// Translate a cell's styling into the renderer's own.
fn style_of(style: CellStyle) -> Style {
    let mut modifiers = Modifier::empty();
    modifiers.set(Modifier::BOLD, style.bold);
    modifiers.set(Modifier::DIM, style.dim);
    modifiers.set(Modifier::ITALIC, style.italic);
    modifiers.set(Modifier::UNDERLINED, style.underline);
    modifiers.set(Modifier::REVERSED, style.inverse);

    Style::default()
        .fg(color_of(style.fg))
        .bg(color_of(style.bg))
        .add_modifier(modifiers)
}

/// Translate one color, keeping the program's intent: an indexed color stays
/// indexed so the user's own palette applies, and a 24-bit color stays exact.
const fn color_of(color: Color) -> UiColor {
    match color {
        Color::Default => UiColor::Reset,
        Color::Idx(index) => UiColor::Indexed(index),
        Color::Rgb(red, green, blue) => UiColor::Rgb(red, green, blue),
    }
}

#[cfg(test)]
mod tests {
    use otag_core::frame::{
        CursorShape, CursorState, InputModes, ScreenFrame, ScreenRow, StyledRun,
    };
    use otag_core::pane::PaneSize;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;

    fn run(text: &str, style: CellStyle) -> StyledRun {
        StyledRun {
            text: text.to_owned(),
            style,
        }
    }

    /// A screen of the given size with the given rows, cursor hidden.
    fn screen(size: PaneSize, rows: Vec<ScreenRow>) -> ScreenState {
        let mut screen = ScreenState::new(size);
        screen.apply(&ScreenFrame {
            size,
            cursor: CursorState::default(),
            modes: InputModes::default(),
            full: true,
            rows,
        });
        screen
    }

    /// Draw a screen into a buffer of the same size.
    fn draw(screen: &ScreenState) -> Buffer {
        let size = screen.size();
        let mut terminal =
            Terminal::new(TestBackend::new(size.cols(), size.rows())).expect("a test terminal");
        terminal
            .draw(|frame| {
                let area = frame.area();
                frame.render_widget(Pane::new(screen), area);
                if let Some(position) = Pane::new(screen).cursor_position(area) {
                    frame.set_cursor_position(position);
                }
            })
            .expect("the screen draws");
        terminal.backend().buffer().clone()
    }

    fn symbols(buffer: &Buffer, row: u16) -> String {
        (0..buffer.area.width)
            .filter_map(|x| buffer.cell((x, row)).map(|cell| cell.symbol().to_owned()))
            .collect()
    }

    #[test]
    fn text_lands_where_the_pane_put_it() {
        let screen = screen(
            PaneSize::new(3, 10),
            vec![
                ScreenRow {
                    index: 0,
                    runs: vec![run("first", CellStyle::default())],
                },
                ScreenRow {
                    index: 2,
                    runs: vec![run("third", CellStyle::default())],
                },
            ],
        );

        let buffer = draw(&screen);
        assert_eq!(symbols(&buffer, 0), "first     ");
        assert_eq!(symbols(&buffer, 1), "          ");
        assert_eq!(symbols(&buffer, 2), "third     ");
    }

    #[test]
    fn colors_and_attributes_reach_the_cells() {
        let style = CellStyle {
            fg: Color::Idx(4),
            bg: Color::Rgb(10, 20, 30),
            bold: true,
            underline: true,
            ..CellStyle::default()
        };
        let screen = screen(
            PaneSize::new(1, 6),
            vec![ScreenRow {
                index: 0,
                runs: vec![run("ab", CellStyle::default()), run("cd", style)],
            }],
        );

        let buffer = draw(&screen);
        let plain = buffer.cell((0, 0)).expect("a cell");
        assert_eq!(plain.fg, UiColor::Reset);
        assert_eq!(plain.bg, UiColor::Reset);
        assert!(!plain.modifier.contains(Modifier::BOLD));

        let styled = buffer.cell((2, 0)).expect("a cell");
        // An indexed color stays indexed, so the user's palette still applies.
        assert_eq!(styled.fg, UiColor::Indexed(4));
        assert_eq!(styled.bg, UiColor::Rgb(10, 20, 30));
        assert!(styled.modifier.contains(Modifier::BOLD));
        assert!(styled.modifier.contains(Modifier::UNDERLINED));
    }

    #[test]
    fn inverse_and_dim_and_italic_are_kept() {
        let screen = screen(
            PaneSize::new(1, 4),
            vec![ScreenRow {
                index: 0,
                runs: vec![run(
                    "x",
                    CellStyle {
                        dim: true,
                        italic: true,
                        inverse: true,
                        ..CellStyle::default()
                    },
                )],
            }],
        );

        let cell = draw(&screen);
        let cell = cell.cell((0, 0)).expect("a cell");
        assert!(cell.modifier.contains(Modifier::DIM));
        assert!(cell.modifier.contains(Modifier::ITALIC));
        assert!(cell.modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn a_double_width_character_takes_two_columns() {
        let style = CellStyle {
            bg: Color::Idx(2),
            ..CellStyle::default()
        };
        let screen = screen(
            PaneSize::new(1, 6),
            vec![ScreenRow {
                index: 0,
                runs: vec![run("\u{4e2d}x", style)],
            }],
        );

        let buffer = draw(&screen);
        // The glyph is written once, in the first of the two columns it
        // covers, and what follows starts after both.
        let wide = buffer.cell((0, 0)).expect("a cell");
        assert_eq!(wide.symbol(), "\u{4e2d}");
        assert_eq!(wide.bg, UiColor::Indexed(2));

        let after = buffer.cell((2, 0)).expect("a cell");
        assert_eq!(after.symbol(), "x");
        assert_eq!(after.bg, UiColor::Indexed(2));
    }

    #[test]
    fn a_row_longer_than_the_area_is_cut_off_rather_than_wrapped() {
        // The pane was drawn for its own size; re-flowing it here would not
        // match what the program believes it drew.
        let screen = screen(
            PaneSize::new(2, 20),
            vec![ScreenRow {
                index: 0,
                runs: vec![run("0123456789abcdefghij", CellStyle::default())],
            }],
        );

        let mut terminal = Terminal::new(TestBackend::new(10, 2)).expect("a test terminal");
        terminal
            .draw(|frame| frame.render_widget(Pane::new(&screen), frame.area()))
            .expect("the screen draws");
        let buffer = terminal.backend().buffer().clone();

        assert_eq!(symbols(&buffer, 0), "0123456789");
        assert_eq!(symbols(&buffer, 1), "          ");
    }

    #[test]
    fn more_rows_than_the_area_are_cut_off() {
        let screen = screen(
            PaneSize::new(4, 4),
            vec![
                ScreenRow {
                    index: 0,
                    runs: vec![run("one", CellStyle::default())],
                },
                ScreenRow {
                    index: 3,
                    runs: vec![run("four", CellStyle::default())],
                },
            ],
        );

        let mut terminal = Terminal::new(TestBackend::new(4, 2)).expect("a test terminal");
        terminal
            .draw(|frame| frame.render_widget(Pane::new(&screen), frame.area()))
            .expect("the screen draws");
        let buffer = terminal.backend().buffer().clone();

        assert_eq!(symbols(&buffer, 0), "one ");
        assert_eq!(symbols(&buffer, 1), "    ");
    }

    #[test]
    fn the_cursor_is_placed_where_the_pane_has_it() {
        let size = PaneSize::new(5, 10);
        let mut screen = ScreenState::new(size);
        screen.apply(&ScreenFrame {
            size,
            cursor: CursorState {
                row: 2,
                col: 4,
                visible: true,
                shape: CursorShape::Default,
            },
            modes: InputModes::default(),
            full: true,
            rows: Vec::new(),
        });

        let area = Rect::new(0, 0, 10, 5);
        assert_eq!(
            Pane::new(&screen).cursor_position(area),
            Some(Position { x: 4, y: 2 })
        );
    }

    #[test]
    fn a_hidden_cursor_is_not_placed() {
        let size = PaneSize::new(5, 10);
        let mut screen = ScreenState::new(size);
        screen.apply(&ScreenFrame {
            size,
            cursor: CursorState {
                row: 1,
                col: 1,
                visible: false,
                shape: CursorShape::Default,
            },
            modes: InputModes::default(),
            full: true,
            rows: Vec::new(),
        });

        assert_eq!(
            Pane::new(&screen).cursor_position(Rect::new(0, 0, 10, 5)),
            None
        );
    }

    #[test]
    fn a_cursor_outside_the_area_is_not_placed() {
        let size = PaneSize::new(30, 100);
        let mut screen = ScreenState::new(size);
        screen.apply(&ScreenFrame {
            size,
            cursor: CursorState {
                row: 25,
                col: 90,
                visible: true,
                shape: CursorShape::Default,
            },
            modes: InputModes::default(),
            full: true,
            rows: Vec::new(),
        });

        // The client's terminal is smaller than the pane for a moment after
        // a resize; a cursor outside it cannot be drawn.
        assert_eq!(
            Pane::new(&screen).cursor_position(Rect::new(0, 0, 10, 5)),
            None
        );
    }

    #[test]
    fn an_empty_screen_draws_nothing_and_does_not_hang() {
        let screen = screen(PaneSize::new(2, 4), Vec::new());
        let buffer = draw(&screen);
        assert_eq!(symbols(&buffer, 0), "    ");
        assert_eq!(symbols(&buffer, 1), "    ");
    }

    #[test]
    fn a_zero_width_area_is_handled() {
        let screen = screen(
            PaneSize::new(1, 4),
            vec![ScreenRow {
                index: 0,
                runs: vec![run("text", CellStyle::default())],
            }],
        );

        let mut buffer = Buffer::empty(Rect::new(0, 0, 0, 0));
        Pane::new(&screen).render(Rect::new(0, 0, 0, 0), &mut buffer);
        assert_eq!(buffer.area.width, 0);
    }
}
