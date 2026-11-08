//! Turning a pane's screen back into terminal output.
//!
//! A client that does its own terminal emulation — the macOS app, through
//! SwiftTerm — wants the pane's bytes, not otag's finished rows. It cannot
//! have the bytes that have already gone past, so attaching in raw mode
//! starts with a *replay*: the escape sequences that put the current screen
//! onto an empty terminal of the same size. After that the daemon forwards
//! the program's own output unchanged.
//!
//! The replay is written to be dull on purpose. It sets the modes the program
//! has turned on, clears the screen, writes every non-empty row from its
//! first column, and puts the cursor back. Nothing here depends on what the
//! receiving terminal had on it before.

use crate::frame::{
    CellStyle, Color, CursorShape, CursorState, InputModes, MouseEncoding, MouseMode, ScreenState,
    StyledRun,
};

/// The escape sequences that recreate `screen`.
///
/// Autowrap is turned off while the rows are written, so a row whose last
/// cell is filled cannot push the next one down, and turned back on at the
/// end because that is the state a terminal starts in.
#[must_use]
pub fn snapshot(screen: &ScreenState) -> Vec<u8> {
    draw(screen, screen.modes()).into_bytes()
}

/// The normal buffer behind a live alternate screen, and its scrollback.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct History {
    pub(crate) lines: Vec<String>,
    pub(crate) normal_screen: Option<ScreenState>,
}

impl History {
    /// Flatten both terminal buffers into the saved scrollback format.
    pub(crate) fn with_visible(mut self, screen: &ScreenState, max: usize) -> Vec<String> {
        if let Some(normal) = &self.normal_screen {
            self.lines
                .extend(normal.text().trim_end().lines().map(str::to_owned));
        }
        self.lines
            .extend(screen.text().trim_end().lines().map(str::to_owned));
        self.lines.drain(..self.lines.len().saturating_sub(max));
        self.lines
    }
}

/// Rebuild the scrollback and screen of a pane on a fresh attachment.
/// A replacement process starts with the old grid in the normal buffer.
pub(crate) fn with_history(screen: &ScreenState, history: &History, retain: bool) -> Vec<u8> {
    let mut out = String::from("\x1b[?1049l\x1b[!p\x1b[0m\x1b[H\x1b[2J\x1b[3J");
    let last_row = screen.size().rows_nonzero();
    out.push_str(&format!("\x1b[{last_row};1H"));
    let mut scroll = |line: &str| {
        out.extend(line.chars().filter(|c| !c.is_control()));
        out.push_str("\r\n");
    };
    for line in &history.lines {
        scroll(line);
    }
    if let Some(normal) = &history.normal_screen {
        if retain {
            // The replacement displays the alternate grid in the normal
            // buffer. Move the old normal grid into scrollback first.
            for line in normal.text().trim_end().lines() {
                scroll(line);
            }
        } else {
            out.push_str(&draw(normal, InputModes::default()));
        }
    }
    out.push_str(&draw(
        screen,
        if retain {
            InputModes::default()
        } else {
            screen.modes()
        },
    ));
    out.into_bytes()
}

fn draw(screen: &ScreenState, input: InputModes) -> String {
    let mut out = String::new();

    // The alternate screen comes first: everything below belongs on whichever
    // buffer the program is drawing on.
    if input.alternate_screen {
        out.push_str("\x1b[?1049h");
    }
    out.push_str("\x1b[0m\x1b[?7l\x1b[H\x1b[2J");

    for (index, runs) in screen.rows().enumerate() {
        if runs.iter().all(is_blank) {
            // A blank row is already blank on a cleared screen.
            continue;
        }
        // Rows and columns are one-based in a cursor position report.
        let row = index.saturating_add(1);
        out.push_str(&format!("\x1b[{row};1H"));
        for run in runs {
            out.push_str(&sgr(run.style));
            out.push_str(&run.text);
        }
        out.push_str("\x1b[0m");
    }

    out.push_str("\x1b[?7h");
    out.push_str(&modes(input));
    out.push_str(&cursor(screen.cursor()));
    out
}

/// Whether a run would leave the screen as a clear already leaves it.
fn is_blank(run: &StyledRun) -> bool {
    run.style == CellStyle::default() && run.text.chars().all(|character| character == ' ')
}

/// The sequences that turn the program's input modes back on.
///
/// Only modes that are on are written: a client's terminal has just been
/// cleared, so everything else is already in its default state.
fn modes(modes: InputModes) -> String {
    let mut out = String::new();
    if modes.application_cursor {
        out.push_str("\x1b[?1h");
    }
    if modes.application_keypad {
        out.push_str("\x1b=");
    }
    if modes.bracketed_paste {
        out.push_str("\x1b[?2004h");
    }
    out.push_str(match modes.mouse {
        MouseMode::None | MouseMode::Unknown => "",
        MouseMode::Press => "\x1b[?9h",
        MouseMode::PressRelease => "\x1b[?1000h",
        MouseMode::ButtonMotion => "\x1b[?1002h",
        MouseMode::AnyMotion => "\x1b[?1003h",
    });
    if modes.mouse.is_on() {
        out.push_str(match modes.mouse_encoding {
            MouseEncoding::Default | MouseEncoding::Unknown => "",
            MouseEncoding::Utf8 => "\x1b[?1005h",
            MouseEncoding::Sgr => "\x1b[?1006h",
        });
    }
    out
}

/// The sequences that put the cursor where the program left it.
fn cursor(cursor: CursorState) -> String {
    let row = cursor.row.saturating_add(1);
    let col = cursor.col.saturating_add(1);
    let mut out = format!("\x1b[{row};{col}H");
    if let Some(shape) = shape(cursor.shape) {
        out.push_str(&format!("\x1b[{shape} q"));
    }
    out.push_str(if cursor.visible {
        "\x1b[?25h"
    } else {
        "\x1b[?25l"
    });
    out
}

/// The `DECSCUSR` parameter for a cursor shape, when it has one.
const fn shape(shape: CursorShape) -> Option<u8> {
    match shape {
        // Asking for the default back is `0`, which is also what a cleared
        // terminal already uses, so nothing is written for it.
        CursorShape::Default | CursorShape::Unknown => None,
        CursorShape::BlinkBlock => Some(1),
        CursorShape::SteadyBlock => Some(2),
        CursorShape::BlinkUnderline => Some(3),
        CursorShape::SteadyUnderline => Some(4),
        CursorShape::BlinkBar => Some(5),
        CursorShape::SteadyBar => Some(6),
    }
}

/// One run's styling, as a complete `SGR` sequence.
///
/// Every run resets first and then sets what it needs, which costs a few
/// bytes once per run and makes the result independent of what came before.
fn sgr(style: CellStyle) -> String {
    let mut params = vec!["0".to_owned()];
    if style.bold {
        params.push("1".to_owned());
    }
    if style.dim {
        params.push("2".to_owned());
    }
    if style.italic {
        params.push("3".to_owned());
    }
    if style.underline {
        params.push("4".to_owned());
    }
    if style.inverse {
        params.push("7".to_owned());
    }
    if let Some(colour) = colour(style.fg, Layer::Foreground) {
        params.push(colour);
    }
    if let Some(colour) = colour(style.bg, Layer::Background) {
        params.push(colour);
    }
    format!("\x1b[{}m", params.join(";"))
}

/// Which half of a cell a colour belongs to.
#[derive(Debug, Clone, Copy)]
enum Layer {
    Foreground,
    Background,
}

impl Layer {
    /// The `SGR` parameter that introduces an extended colour.
    const fn extended(self) -> u8 {
        match self {
            Self::Foreground => 38,
            Self::Background => 48,
        }
    }
}

/// One colour as `SGR` parameters, or nothing when it is the terminal's own.
fn colour(colour: Color, layer: Layer) -> Option<String> {
    let extended = layer.extended();
    match colour {
        Color::Default => None,
        Color::Idx(index) => Some(format!("{extended};5;{index}")),
        Color::Rgb(red, green, blue) => Some(format!("{extended};2;{red};{green};{blue}")),
    }
}

#[cfg(test)]
mod tests {
    use crate::pane::PaneSize;

    use super::*;

    /// A screen with these rows, at this size.
    fn screen(rows: Vec<Vec<StyledRun>>, cursor: CursorState, modes: InputModes) -> ScreenState {
        let size = PaneSize::new(u16::try_from(rows.len().max(1)).unwrap_or(1), 80);
        ScreenState::from_parts(size, cursor, modes, rows)
    }

    fn run(text: &str, style: CellStyle) -> StyledRun {
        StyledRun {
            text: text.to_owned(),
            style,
        }
    }

    /// The replay as text, which is what the assertions read.
    fn replayed(screen: &ScreenState) -> String {
        String::from_utf8(snapshot(screen)).expect("the replay is text")
    }

    #[test]
    fn a_replay_clears_the_screen_before_writing_anything() {
        let text = replayed(&screen(
            vec![vec![run("hello", CellStyle::default())]],
            CursorState::default(),
            InputModes::default(),
        ));

        let clear = text.find("\x1b[2J").expect("the screen is cleared");
        let hello = text.find("hello").expect("the row is written");
        assert!(clear < hello, "the clear comes first: {text:?}");
    }

    #[test]
    fn every_row_is_written_at_its_own_one_based_position() {
        let text = replayed(&screen(
            vec![
                vec![run("first", CellStyle::default())],
                Vec::new(),
                vec![run("third", CellStyle::default())],
            ],
            CursorState::default(),
            InputModes::default(),
        ));

        assert!(text.contains("\x1b[1;1H\x1b[0mfirst"), "{text:?}");
        assert!(text.contains("\x1b[3;1H\x1b[0mthird"), "{text:?}");
        // The blank row in the middle is left to the clear.
        assert!(!text.contains("\x1b[2;1H"), "{text:?}");
    }

    #[test]
    fn a_row_of_blanks_in_the_default_style_is_not_written() {
        let blanks = replayed(&screen(
            vec![vec![run("    ", CellStyle::default())]],
            CursorState::default(),
            InputModes::default(),
        ));
        let empty = replayed(&screen(
            vec![Vec::new()],
            CursorState::default(),
            InputModes::default(),
        ));
        assert_eq!(blanks, empty);
    }

    #[test]
    fn a_row_of_blanks_with_a_background_is_written() {
        // A filled background is part of what the screen looks like.
        let text = replayed(&screen(
            vec![vec![run(
                "  ",
                CellStyle {
                    bg: Color::Idx(4),
                    ..CellStyle::default()
                },
            )]],
            CursorState::default(),
            InputModes::default(),
        ));
        assert!(text.contains("\x1b[0;48;5;4m  "), "{text:?}");
    }

    #[test]
    fn colours_and_attributes_are_written_as_sgr() {
        let text = replayed(&screen(
            vec![vec![run(
                "x",
                CellStyle {
                    fg: Color::Rgb(1, 2, 3),
                    bg: Color::Idx(7),
                    bold: true,
                    dim: true,
                    italic: true,
                    underline: true,
                    inverse: true,
                },
            )]],
            CursorState::default(),
            InputModes::default(),
        ));
        assert!(
            text.contains("\x1b[0;1;2;3;4;7;38;2;1;2;3;48;5;7mx"),
            "{text:?}"
        );
    }

    #[test]
    fn a_run_in_the_default_style_names_no_colour() {
        let text = replayed(&screen(
            vec![vec![run("x", CellStyle::default())]],
            CursorState::default(),
            InputModes::default(),
        ));
        assert!(text.contains("\x1b[0mx"), "{text:?}");
    }

    #[test]
    fn the_cursor_ends_where_the_program_left_it() {
        let text = replayed(&screen(
            vec![Vec::new(), Vec::new(), Vec::new()],
            CursorState {
                row: 2,
                col: 4,
                visible: true,
                shape: CursorShape::SteadyBar,
            },
            InputModes::default(),
        ));

        assert!(text.ends_with("\x1b[3;5H\x1b[6 q\x1b[?25h"), "{text:?}");
    }

    #[test]
    fn a_hidden_cursor_stays_hidden() {
        let text = replayed(&screen(
            vec![Vec::new()],
            CursorState::default(),
            InputModes::default(),
        ));
        assert!(text.ends_with("\x1b[?25l"), "{text:?}");
    }

    #[test]
    fn the_alternate_screen_is_entered_before_the_screen_is_drawn() {
        let text = replayed(&screen(
            vec![vec![run("vim", CellStyle::default())]],
            CursorState::default(),
            InputModes {
                alternate_screen: true,
                ..InputModes::default()
            },
        ));
        assert!(text.starts_with("\x1b[?1049h"), "{text:?}");
    }

    #[test]
    fn the_modes_the_program_turned_on_are_turned_on_again() {
        let text = replayed(&screen(
            vec![Vec::new()],
            CursorState::default(),
            InputModes {
                application_cursor: true,
                application_keypad: true,
                bracketed_paste: true,
                alternate_screen: false,
                mouse: MouseMode::ButtonMotion,
                mouse_encoding: MouseEncoding::Sgr,
            },
        ));

        for sequence in [
            "\x1b[?1h",
            "\x1b=",
            "\x1b[?2004h",
            "\x1b[?1002h",
            "\x1b[?1006h",
        ] {
            assert!(
                text.contains(sequence),
                "{sequence:?} missing from {text:?}"
            );
        }
    }

    #[test]
    fn a_mode_that_is_off_is_not_mentioned() {
        let text = replayed(&screen(
            vec![Vec::new()],
            CursorState::default(),
            InputModes::default(),
        ));
        for sequence in ["\x1b[?1h", "\x1b=", "\x1b[?2004h", "\x1b[?1049h"] {
            assert!(!text.contains(sequence), "{sequence:?} is in {text:?}");
        }
    }

    #[test]
    fn autowrap_is_off_while_the_rows_are_written_and_on_again_afterwards() {
        // A row whose last cell is filled must not push the next row down.
        let text = replayed(&screen(
            vec![vec![run("x", CellStyle::default())]],
            CursorState::default(),
            InputModes::default(),
        ));

        let off = text.find("\x1b[?7l").expect("autowrap is turned off");
        let row = text.find("\x1b[1;1H").expect("the row is written");
        let on = text.find("\x1b[?7h").expect("autowrap is turned back on");
        assert!(off < row && row < on, "{text:?}");
    }

    #[test]
    fn an_empty_screen_replays_to_a_clear_and_a_cursor() {
        let screen = ScreenState::new(PaneSize::new(5, 10));
        let text = replayed(&screen);
        assert_eq!(
            text,
            "\x1b[0m\x1b[?7l\x1b[H\x1b[2J\x1b[?7h\x1b[1;1H\x1b[?25l"
        );
    }

    #[test]
    fn a_double_width_character_is_written_once() {
        let text = replayed(&screen(
            vec![vec![run("\u{4e2d}x", CellStyle::default())]],
            CursorState::default(),
            InputModes::default(),
        ));
        assert!(text.contains("\u{4e2d}x"), "{text:?}");
    }
}
