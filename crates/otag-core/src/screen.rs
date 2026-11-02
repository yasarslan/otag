//! Terminal emulation for one pane.
//!
//! [`Screen`] feeds a pane's output into a terminal emulator and turns the
//! resulting grid into the [`ScreenState`] a client draws. Keeping the
//! emulator in the daemon means a pane's content survives a client going
//! away, and that every feature built later reads the same screen the user
//! sees.

use crate::frame::{
    CellStyle, Color, CursorShape, CursorState, InputModes, MouseEncoding, MouseMode, ScreenState,
    StyledRun,
};
use crate::pane::PaneSize;

/// How many lines of scrolled-off output a pane keeps.
///
/// Scrollback is not shown yet; the buffer exists so that the lines are there
/// when it is.
pub const DEFAULT_SCROLLBACK: usize = 1_000;

/// Largest amount of unread answers to device queries otag keeps.
///
/// A program that asks faster than the daemon drains the buffer is
/// misbehaving; dropping its queries is better than growing without bound.
const MAX_PENDING_REPLIES: usize = 4 * 1024;

/// The emulated screen of one pane.
pub struct Screen {
    parser: vt100::Parser<Responder>,
    size: PaneSize,
}

impl Screen {
    /// An empty screen of the given size.
    #[must_use]
    pub fn new(size: PaneSize) -> Self {
        Self::with_scrollback(size, DEFAULT_SCROLLBACK)
    }

    /// An empty screen that keeps `scrollback` lines of scrolled-off output.
    #[must_use]
    pub fn with_scrollback(size: PaneSize, scrollback: usize) -> Self {
        Self {
            parser: vt100::Parser::new_with_callbacks(
                size.rows_nonzero(),
                size.cols_nonzero(),
                scrollback,
                Responder::default(),
            ),
            size,
        }
    }

    /// Current size.
    #[must_use]
    pub const fn size(&self) -> PaneSize {
        self.size
    }

    /// Feed output from the pane's program into the emulator.
    pub fn process(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
    }

    /// Resize the screen, keeping as much content as the new size allows.
    pub fn resize(&mut self, size: PaneSize) {
        if size == self.size {
            return;
        }
        self.parser
            .set_size(size.rows_nonzero(), size.cols_nonzero());
        self.size = size;
    }

    /// Answers the emulator owes the pane's program, for example the reply to
    /// a cursor-position query. Reading them clears the buffer; the caller
    /// writes them to the pane.
    pub fn take_replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.parser.callbacks_mut().replies)
    }

    /// The screen as the client should see it.
    #[must_use]
    pub fn state(&self) -> ScreenState {
        let screen = self.parser.screen();
        let (cursor_row, cursor_col) = screen.cursor_position();

        let cursor = CursorState {
            row: cursor_row,
            col: cursor_col,
            visible: !screen.hide_cursor(),
            shape: cursor_shape(screen.cursor_style()),
        };
        let modes = InputModes {
            application_cursor: screen.application_cursor(),
            application_keypad: screen.application_keypad(),
            bracketed_paste: screen.bracketed_paste(),
            alternate_screen: screen.alternate_screen(),
            mouse: mouse_mode(screen.mouse_protocol_mode()),
            mouse_encoding: mouse_encoding(screen.mouse_protocol_encoding()),
        };
        let rows = (0..self.size.rows())
            .map(|index| row_runs(screen, index, self.size.cols()))
            .collect();

        ScreenState::from_parts(self.size, cursor, modes, rows)
    }
}

/// Answers the device queries programs expect a terminal to reply to.
///
/// The emulator itself only tracks screen state, so these replies are
/// produced here. Without them a program that asks what kind of terminal it
/// is talking to waits for its own timeout before drawing, which is visible
/// as a pause when an editor starts.
#[derive(Debug, Default)]
struct Responder {
    replies: Vec<u8>,
}

impl Responder {
    fn reply(&mut self, bytes: &[u8]) {
        if self.replies.len().saturating_add(bytes.len()) > MAX_PENDING_REPLIES {
            tracing::debug!("dropping a terminal reply: the pane is not reading its input");
            return;
        }
        self.replies.extend_from_slice(bytes);
    }
}

impl vt100::Callbacks for Responder {
    fn resize(&mut self, _screen: &mut vt100::Screen, request: (u16, u16)) {
        // A program may ask to resize the window. otag's panes are sized by
        // the user's terminal and the layout, so the request is ignored.
        tracing::debug!(
            rows = request.0,
            cols = request.1,
            "ignoring a resize request from a pane"
        );
    }

    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        intermediate: Option<u8>,
        _second: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        let first = params.first().and_then(|param| param.first().copied());
        match (intermediate, c) {
            // Device status report: terminal health, then cursor position.
            (None, 'n') => match first {
                Some(5) => self.reply(b"\x1b[0n"),
                Some(6) => {
                    let (row, col) = screen.cursor_position();
                    // The report is one-based.
                    let reply =
                        format!("\x1b[{};{}R", row.saturating_add(1), col.saturating_add(1));
                    self.reply(reply.as_bytes());
                }
                _ => {}
            },
            // Primary device attributes: a VT100 with an advanced video
            // option, which is what otag's emulation offers.
            (None, 'c') if matches!(first, None | Some(0)) => self.reply(b"\x1b[?1;2c"),
            // Secondary device attributes: terminal type 0, firmware 10.
            (Some(b'>'), 'c') => self.reply(b"\x1b[>0;10;1c"),
            // Report the text area size in characters.
            (None, 't') if first == Some(18) => {
                let (rows, cols) = screen.size();
                let reply = format!("\x1b[8;{};{}t", rows.get(), cols.get());
                self.reply(reply.as_bytes());
            }
            _ => {}
        }
    }
}

impl std::fmt::Debug for Screen {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Screen")
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

/// Build the runs of one row, merging neighbouring cells that are drawn the
/// same way and dropping the unstyled blanks at the end.
fn row_runs(screen: &vt100::Screen, row: u16, cols: u16) -> Vec<StyledRun> {
    let mut runs: Vec<StyledRun> = Vec::new();

    for col in 0..cols {
        let Some(cell) = screen.cell(row, col) else {
            continue;
        };
        // The second half of a double-width character holds no content of its
        // own: the character was already emitted by the cell before it.
        if cell.is_wide_continuation() {
            continue;
        }

        let style = cell_style(cell);
        // A blank cell still paints its background, so it becomes a space.
        let contents = cell.contents();
        let text = if contents.is_empty() { " " } else { contents };

        match runs.last_mut() {
            Some(last) if last.style == style => last.text.push_str(text),
            _ => runs.push(StyledRun {
                text: text.to_owned(),
                style,
            }),
        }
    }

    trim_trailing_blanks(&mut runs);
    runs
}

/// Drop the unstyled blanks at the end of a row.
///
/// Applying a row blanks it first, so trailing blanks do not have to be sent.
/// A blank with a background color is not blank to look at and stays.
fn trim_trailing_blanks(runs: &mut Vec<StyledRun>) {
    while let Some(last) = runs.last_mut() {
        if last.style != CellStyle::default() {
            return;
        }
        let kept = last.text.trim_end_matches(' ').len();
        if kept == last.text.len() {
            return;
        }
        last.text.truncate(kept);
        if !last.text.is_empty() {
            return;
        }
        runs.pop();
    }
}

fn cell_style(cell: &vt100::Cell) -> CellStyle {
    CellStyle {
        fg: color(cell.fgcolor()),
        bg: color(cell.bgcolor()),
        bold: cell.bold(),
        dim: cell.dim(),
        italic: cell.italic(),
        underline: cell.underline(),
        inverse: cell.inverse(),
    }
}

fn color(color: vt100::Color) -> Color {
    match color {
        vt100::Color::Default => Color::Default,
        vt100::Color::Idx(index) => Color::Idx(index),
        vt100::Color::Rgb(red, green, blue) => Color::Rgb(red, green, blue),
    }
}

fn cursor_shape(style: vt100::CursorStyle) -> CursorShape {
    match style {
        vt100::CursorStyle::Default => CursorShape::Default,
        vt100::CursorStyle::BlinkBlock => CursorShape::BlinkBlock,
        vt100::CursorStyle::SteadyBlock => CursorShape::SteadyBlock,
        vt100::CursorStyle::BlinkUnderline => CursorShape::BlinkUnderline,
        vt100::CursorStyle::SteadyUnderline => CursorShape::SteadyUnderline,
        vt100::CursorStyle::BlinkVerticalBar => CursorShape::BlinkBar,
        vt100::CursorStyle::SteadyVerticalBar => CursorShape::SteadyBar,
    }
}

fn mouse_mode(mode: vt100::MouseProtocolMode) -> MouseMode {
    match mode {
        vt100::MouseProtocolMode::None => MouseMode::None,
        vt100::MouseProtocolMode::Press => MouseMode::Press,
        vt100::MouseProtocolMode::PressRelease => MouseMode::PressRelease,
        vt100::MouseProtocolMode::ButtonMotion => MouseMode::ButtonMotion,
        vt100::MouseProtocolMode::AnyMotion => MouseMode::AnyMotion,
    }
}

fn mouse_encoding(encoding: vt100::MouseProtocolEncoding) -> MouseEncoding {
    match encoding {
        vt100::MouseProtocolEncoding::Default => MouseEncoding::Default,
        vt100::MouseProtocolEncoding::Utf8 => MouseEncoding::Utf8,
        vt100::MouseProtocolEncoding::Sgr => MouseEncoding::Sgr,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{MouseEncoding, MouseMode};

    fn emulate(rows: u16, cols: u16, output: &str) -> Screen {
        let mut screen = Screen::new(PaneSize::new(rows, cols));
        screen.process(output.as_bytes());
        screen
    }

    #[test]
    fn plain_output_becomes_screen_text() {
        let screen = emulate(3, 20, "first\r\nsecond");
        let state = screen.state();

        assert_eq!(state.row_text(0), "first");
        assert_eq!(state.row_text(1), "second");
        assert_eq!(state.row_text(2), "");
        assert_eq!(state.size(), PaneSize::new(3, 20));
    }

    #[test]
    fn a_blank_row_has_no_runs_at_all() {
        let screen = emulate(2, 20, "text");
        assert!(screen.state().row(1).is_empty());
    }

    #[test]
    fn styled_text_becomes_separate_runs() {
        // Default, then bold red, then default again.
        let screen = emulate(1, 20, "ab\x1b[1;31mcd\x1b[0mef");
        let state = screen.state();
        let runs = state.row(0);

        assert_eq!(runs.len(), 3);
        assert_eq!(runs[0].text, "ab");
        assert_eq!(runs[0].style, CellStyle::default());
        assert_eq!(runs[1].text, "cd");
        assert!(runs[1].style.bold);
        assert_eq!(runs[1].style.fg, Color::Idx(1));
        assert_eq!(runs[2].text, "ef");
        assert_eq!(runs[2].style, CellStyle::default());
    }

    #[test]
    fn every_attribute_survives_the_trip() {
        let screen = emulate(1, 20, "\x1b[1;3;4;7mx");
        let style = screen.state().row(0)[0].style;

        assert!(style.bold);
        assert!(style.italic);
        assert!(style.underline);
        assert!(style.inverse);
        // Bold and dim are one intensity, as on a real terminal: the later
        // one replaces the earlier.
        assert!(!style.dim);

        let dim = emulate(1, 20, "\x1b[1;2mx");
        assert!(dim.state().row(0)[0].style.dim);
        assert!(!dim.state().row(0)[0].style.bold);
    }

    #[test]
    fn indexed_and_rgb_colors_are_kept_as_they_were_sent() {
        let screen = emulate(1, 30, "\x1b[38;5;214ma\x1b[38;2;10;20;30;48;5;99mb");
        let state = screen.state();
        let runs = state.row(0);

        assert_eq!(runs[0].style.fg, Color::Idx(214));
        assert_eq!(runs[1].style.fg, Color::Rgb(10, 20, 30));
        assert_eq!(runs[1].style.bg, Color::Idx(99));
    }

    #[test]
    fn a_background_color_keeps_its_blanks() {
        // A blank cell with a background still has to be painted, so it
        // cannot be trimmed away like an unstyled blank.
        let screen = emulate(1, 10, "\x1b[41m    \x1b[0m");
        let state = screen.state();
        let runs = state.row(0);

        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].text, "    ");
        assert_eq!(runs[0].style.bg, Color::Idx(1));
    }

    #[test]
    fn a_double_width_character_appears_once() {
        let screen = emulate(1, 10, "ab\u{4e2d}cd");
        let state = screen.state();

        // Five characters across six columns, listed once each.
        assert_eq!(state.row_text(0), "ab\u{4e2d}cd");
        let total: usize = state
            .row(0)
            .iter()
            .map(|run| run.text.chars().count())
            .sum();
        assert_eq!(total, 5);
    }

    #[test]
    fn combining_characters_stay_with_their_cell() {
        let screen = emulate(1, 10, "e\u{301}x");
        assert_eq!(screen.state().row_text(0), "e\u{301}x");
    }

    #[test]
    fn the_cursor_position_and_visibility_are_reported() {
        let screen = emulate(5, 20, "\x1b[3;7H");
        let cursor = screen.state().cursor();
        // The escape sequence is one-based, the frame is zero-based.
        assert_eq!((cursor.row, cursor.col), (2, 6));
        assert!(cursor.visible);

        let hidden = emulate(5, 20, "\x1b[?25l");
        assert!(!hidden.state().cursor().visible);
    }

    #[test]
    fn the_cursor_shape_is_reported() {
        let screen = emulate(2, 10, "\x1b[5 q");
        assert_eq!(screen.state().cursor().shape, CursorShape::BlinkBar);
    }

    #[test]
    fn application_cursor_mode_is_tracked() {
        let mut screen = emulate(2, 10, "\x1b[?1h");
        assert!(screen.state().modes().application_cursor);

        screen.process(b"\x1b[?1l");
        assert!(!screen.state().modes().application_cursor);
    }

    #[test]
    fn application_keypad_mode_is_tracked() {
        let mut screen = emulate(2, 10, "\x1b=");
        assert!(screen.state().modes().application_keypad);

        screen.process(b"\x1b>");
        assert!(!screen.state().modes().application_keypad);
    }

    #[test]
    fn bracketed_paste_mode_is_tracked() {
        let mut screen = emulate(2, 10, "\x1b[?2004h");
        assert!(screen.state().modes().bracketed_paste);

        screen.process(b"\x1b[?2004l");
        assert!(!screen.state().modes().bracketed_paste);
    }

    #[test]
    fn the_alternate_screen_is_tracked_and_keeps_the_main_screen_separate() {
        let mut screen = emulate(2, 20, "main text");
        assert!(!screen.state().modes().alternate_screen);

        screen.process(b"\x1b[?1049h");
        assert!(screen.state().modes().alternate_screen);
        assert_eq!(screen.state().row_text(0), "");

        screen.process(b"alternate");
        assert_eq!(screen.state().row_text(0), "alternate");

        screen.process(b"\x1b[?1049l");
        assert!(!screen.state().modes().alternate_screen);
        assert_eq!(screen.state().row_text(0), "main text");
    }

    #[test]
    fn mouse_reporting_is_tracked() {
        let mut screen = emulate(2, 10, "");
        assert_eq!(screen.state().modes().mouse, MouseMode::None);
        assert!(!screen.state().modes().mouse.is_on());

        screen.process(b"\x1b[?1002h\x1b[?1006h");
        assert_eq!(screen.state().modes().mouse, MouseMode::ButtonMotion);
        assert_eq!(screen.state().modes().mouse_encoding, MouseEncoding::Sgr);

        screen.process(b"\x1b[?1002l");
        assert_eq!(screen.state().modes().mouse, MouseMode::None);
    }

    #[test]
    fn scroll_regions_and_line_insertion_work() {
        // What a full-screen program relies on when it scrolls part of the
        // screen: a scroll region plus insert and delete line.
        let mut screen = emulate(4, 10, "one\r\ntwo\r\nthree\r\nfour");
        screen.process(b"\x1b[2;3r\x1b[2;1H\x1b[M");

        let state = screen.state();
        assert_eq!(state.row_text(0), "one");
        assert_eq!(state.row_text(1), "three");
        assert_eq!(state.row_text(2), "");
        assert_eq!(state.row_text(3), "four");
    }

    #[test]
    fn resizing_keeps_the_content_and_the_new_size() {
        let mut screen = emulate(4, 20, "kept");
        screen.resize(PaneSize::new(10, 40));

        assert_eq!(screen.size(), PaneSize::new(10, 40));
        let state = screen.state();
        assert_eq!(state.size(), PaneSize::new(10, 40));
        assert_eq!(state.row_text(0), "kept");
        assert_eq!(state.rows().count(), 10);
    }

    #[test]
    fn resizing_to_the_same_size_changes_nothing() {
        let mut screen = emulate(3, 10, "same");
        let before = screen.state();
        screen.resize(PaneSize::new(3, 10));
        assert_eq!(screen.state(), before);
    }

    #[test]
    fn a_cursor_position_query_is_answered() {
        let mut screen = emulate(10, 40, "\x1b[4;9H\x1b[6n");
        // The answer is one-based, like the query.
        assert_eq!(screen.take_replies(), b"\x1b[4;9R");
        // Reading the answers clears them.
        assert!(screen.take_replies().is_empty());
    }

    #[test]
    fn a_status_query_is_answered() {
        let mut screen = emulate(5, 10, "\x1b[5n");
        assert_eq!(screen.take_replies(), b"\x1b[0n");
    }

    #[test]
    fn device_attribute_queries_are_answered() {
        let mut primary = emulate(5, 10, "\x1b[c");
        assert_eq!(primary.take_replies(), b"\x1b[?1;2c");

        let mut explicit = emulate(5, 10, "\x1b[0c");
        assert_eq!(explicit.take_replies(), b"\x1b[?1;2c");

        let mut secondary = emulate(5, 10, "\x1b[>c");
        assert_eq!(secondary.take_replies(), b"\x1b[>0;10;1c");
    }

    #[test]
    fn a_text_area_size_query_is_answered() {
        let mut screen = emulate(24, 80, "\x1b[18t");
        assert_eq!(screen.take_replies(), b"\x1b[8;24;80t");
    }

    #[test]
    fn a_program_cannot_resize_the_users_terminal() {
        let mut screen = emulate(5, 10, "\x1b[8;50;200t");
        assert_eq!(screen.size(), PaneSize::new(5, 10));
        assert!(screen.take_replies().is_empty());
    }

    #[test]
    fn unanswerable_queries_are_ignored_quietly() {
        let mut screen = emulate(5, 10, "\x1b[?999n\x1b[42;43;44~\x1b]9999;junk\x07");
        assert!(screen.take_replies().is_empty());
        assert_eq!(screen.state().row_text(0), "");
    }

    #[test]
    fn a_flood_of_queries_cannot_grow_without_bound() {
        let query = "\x1b[6n".repeat(4_000);
        let mut screen = emulate(5, 10, &query);
        assert!(screen.take_replies().len() <= MAX_PENDING_REPLIES);
    }

    #[test]
    fn invalid_output_does_not_disturb_the_screen() {
        // Broken escape sequences and invalid UTF-8 arrive from real
        // programs; neither may lose the screen.
        let mut screen = Screen::new(PaneSize::new(2, 20));
        screen.process(b"\x1b[\x1b[999;999H\xff\xfe");
        // The screen still works: a cursor move and a write both land.
        screen.process(b"\x1b[H\x1b[2Jtext");
        assert_eq!(screen.state().row_text(0), "text");
    }
}
