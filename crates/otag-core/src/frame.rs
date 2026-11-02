//! What a pane's screen looks like, in a form that crosses the socket.
//!
//! The daemon owns the terminal emulation and sends the client finished
//! screen content rather than raw terminal output, so that emulation happens
//! in exactly one place. A [`ScreenFrame`] is either a complete screen or the
//! rows that changed since the last one; [`ScreenState`] applies a sequence of
//! them and is what a renderer draws.
//!
//! Rows are stored as runs of equally styled text, which keeps a frame small
//! without the client having to understand terminal escape sequences.

use serde::{Deserialize, Serialize};

use crate::pane::PaneSize;

/// A terminal color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Color {
    /// The terminal's own foreground or background color.
    #[default]
    Default,
    /// One of the 256 indexed colors.
    Idx(u8),
    /// A 24-bit color.
    Rgb(u8, u8, u8),
}

impl Color {
    /// Whether this is the terminal's default color.
    #[must_use]
    pub const fn is_default(&self) -> bool {
        matches!(self, Self::Default)
    }
}

/// How a run of text is drawn.
///
/// Attributes that are off are left out of the encoded form, which matters:
/// most cells on a screen are unstyled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CellStyle {
    /// Foreground color.
    #[serde(default, skip_serializing_if = "Color::is_default")]
    pub fg: Color,
    /// Background color.
    #[serde(default, skip_serializing_if = "Color::is_default")]
    pub bg: Color,
    /// Bold text.
    #[serde(default, skip_serializing_if = "is_false")]
    pub bold: bool,
    /// Dim text.
    #[serde(default, skip_serializing_if = "is_false")]
    pub dim: bool,
    /// Italic text.
    #[serde(default, skip_serializing_if = "is_false")]
    pub italic: bool,
    /// Underlined text.
    #[serde(default, skip_serializing_if = "is_false")]
    pub underline: bool,
    /// Foreground and background swapped.
    #[serde(default, skip_serializing_if = "is_false")]
    pub inverse: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// A run of consecutive cells drawn the same way.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StyledRun {
    /// Text of the run. One entry per cell; a double-width character occupies
    /// two columns but appears once.
    pub text: String,
    /// How the run is drawn.
    #[serde(default, skip_serializing_if = "is_unstyled")]
    pub style: CellStyle,
}

fn is_unstyled(style: &CellStyle) -> bool {
    *style == CellStyle::default()
}

/// One row of a screen.
///
/// A row carries its own index, because a frame usually contains only the
/// rows that changed. Trailing unstyled blanks are left out; applying a row
/// blanks it first, so what is missing is empty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenRow {
    /// Zero-based position of this row on the screen.
    pub index: u16,
    /// Content of the row, left to right.
    #[serde(default)]
    pub runs: Vec<StyledRun>,
}

/// The shape a terminal should give the cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorShape {
    /// Whatever the user's terminal uses by default.
    #[default]
    Default,
    /// A blinking block.
    BlinkBlock,
    /// A steady block.
    SteadyBlock,
    /// A blinking underline.
    BlinkUnderline,
    /// A steady underline.
    SteadyUnderline,
    /// A blinking vertical bar.
    BlinkBar,
    /// A steady vertical bar.
    SteadyBar,
    /// A shape this build does not know about.
    #[serde(other)]
    Unknown,
}

/// Where the cursor is and how it looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CursorState {
    /// Zero-based row.
    pub row: u16,
    /// Zero-based column.
    pub col: u16,
    /// Whether the cursor should be drawn at all.
    pub visible: bool,
    /// Shape the program asked for.
    #[serde(default)]
    pub shape: CursorShape,
}

/// Mouse reporting a program has turned on.
///
/// otag does not forward mouse events to panes yet, but the mode travels with
/// every frame so that the client can tell whether a pane wants them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseMode {
    /// No mouse reporting.
    #[default]
    None,
    /// Button presses only.
    Press,
    /// Button presses and releases.
    PressRelease,
    /// Presses, releases and motion while a button is held.
    ButtonMotion,
    /// Presses, releases and all motion.
    AnyMotion,
    /// A mode this build does not know about.
    #[serde(other)]
    Unknown,
}

impl MouseMode {
    /// Whether any mouse reporting is on.
    #[must_use]
    pub const fn is_on(self) -> bool {
        !matches!(self, Self::None)
    }
}

/// How mouse events are encoded when they are reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseEncoding {
    /// The original single-byte encoding.
    #[default]
    Default,
    /// The UTF-8 encoding.
    Utf8,
    /// The SGR encoding.
    Sgr,
    /// An encoding this build does not know about.
    #[serde(other)]
    Unknown,
}

/// Input modes a pane's program has turned on.
///
/// These decide how a key press must be encoded before it is written to the
/// pane: an arrow key, for example, is `ESC [ A` normally but `ESC O A` once a
/// program such as an editor turns on application cursor keys. The client
/// cannot know this on its own, so every frame carries the current modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct InputModes {
    /// Cursor keys send `ESC O` sequences instead of `ESC [` sequences.
    #[serde(default, skip_serializing_if = "is_false")]
    pub application_cursor: bool,
    /// The numeric keypad sends application sequences.
    #[serde(default, skip_serializing_if = "is_false")]
    pub application_keypad: bool,
    /// Pasted text must be wrapped in paste markers.
    #[serde(default, skip_serializing_if = "is_false")]
    pub bracketed_paste: bool,
    /// The program is drawing on the alternate screen.
    #[serde(default, skip_serializing_if = "is_false")]
    pub alternate_screen: bool,
    /// Mouse reporting the program asked for.
    #[serde(default, skip_serializing_if = "mouse_is_off")]
    pub mouse: MouseMode,
    /// Encoding for reported mouse events.
    #[serde(default, skip_serializing_if = "mouse_encoding_is_default")]
    pub mouse_encoding: MouseEncoding,
}

fn mouse_is_off(mode: &MouseMode) -> bool {
    !mode.is_on()
}

fn mouse_encoding_is_default(encoding: &MouseEncoding) -> bool {
    matches!(encoding, MouseEncoding::Default)
}

/// A screen update for one pane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenFrame {
    /// Size of the screen this frame describes.
    pub size: PaneSize,
    /// Cursor position and shape after the update.
    pub cursor: CursorState,
    /// Input modes after the update.
    #[serde(default)]
    pub modes: InputModes,
    /// Whether `rows` is the whole screen. A full frame replaces everything
    /// the client has; otherwise only the rows listed are replaced.
    #[serde(default)]
    pub full: bool,
    /// Rows that this frame sets.
    #[serde(default)]
    pub rows: Vec<ScreenRow>,
}

/// A screen assembled from the frames received so far.
///
/// The daemon keeps one to know what each client has already been sent; a
/// client keeps one as the thing it draws.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenState {
    size: PaneSize,
    cursor: CursorState,
    modes: InputModes,
    rows: Vec<Vec<StyledRun>>,
}

impl ScreenState {
    /// An empty screen of the given size.
    #[must_use]
    pub fn new(size: PaneSize) -> Self {
        Self {
            size,
            cursor: CursorState::default(),
            modes: InputModes::default(),
            rows: vec![Vec::new(); usize::from(size.rows())],
        }
    }

    /// A screen built from already-assembled parts.
    ///
    /// Rows beyond the size are dropped and missing rows are blank, so the
    /// result always matches `size`.
    #[must_use]
    pub fn from_parts(
        size: PaneSize,
        cursor: CursorState,
        modes: InputModes,
        mut rows: Vec<Vec<StyledRun>>,
    ) -> Self {
        rows.resize(usize::from(size.rows()), Vec::new());
        Self {
            size,
            cursor,
            modes,
            rows,
        }
    }

    /// Current size.
    #[must_use]
    pub const fn size(&self) -> PaneSize {
        self.size
    }

    /// Current cursor position and shape.
    #[must_use]
    pub const fn cursor(&self) -> CursorState {
        self.cursor
    }

    /// Input modes the pane's program has turned on.
    #[must_use]
    pub const fn modes(&self) -> InputModes {
        self.modes
    }

    /// The runs making up row `index`, empty when the row is blank.
    #[must_use]
    pub fn row(&self, index: u16) -> &[StyledRun] {
        self.rows
            .get(usize::from(index))
            .map_or(&[], |runs| runs.as_slice())
    }

    /// Every row, top to bottom.
    pub fn rows(&self) -> impl Iterator<Item = &[StyledRun]> {
        self.rows.iter().map(Vec::as_slice)
    }

    /// The text of row `index` without styling, with trailing blanks removed.
    #[must_use]
    pub fn row_text(&self, index: u16) -> String {
        let mut text = String::new();
        for run in self.row(index) {
            text.push_str(&run.text);
        }
        text.trim_end().to_owned()
    }

    /// The whole screen as plain text, one line per row.
    #[must_use]
    pub fn text(&self) -> String {
        (0..self.size.rows())
            .map(|index| self.row_text(index))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Apply a frame.
    ///
    /// A frame that changes the size, or that is marked full, replaces the
    /// whole screen; otherwise the listed rows are replaced and the rest is
    /// kept. Rows outside the screen are ignored rather than rejected, so a
    /// frame from a newer daemon cannot put the client into a broken state.
    pub fn apply(&mut self, frame: &ScreenFrame) {
        if frame.full || frame.size != self.size {
            self.size = frame.size;
            self.rows.clear();
            self.rows.resize(usize::from(frame.size.rows()), Vec::new());
        }
        for row in &frame.rows {
            if let Some(slot) = self.rows.get_mut(usize::from(row.index)) {
                slot.clone_from(&row.runs);
            }
        }
        self.cursor = frame.cursor;
        self.modes = frame.modes;
    }

    /// The whole screen as one full frame.
    #[must_use]
    pub fn to_frame(&self) -> ScreenFrame {
        ScreenFrame {
            size: self.size,
            cursor: self.cursor,
            modes: self.modes,
            full: true,
            rows: self
                .rows
                .iter()
                .enumerate()
                .filter(|(_, runs)| !runs.is_empty())
                .map(|(index, runs)| ScreenRow {
                    // A screen never has more rows than `PaneSize` allows, so
                    // the index always fits.
                    index: u16::try_from(index).unwrap_or(u16::MAX),
                    runs: runs.clone(),
                })
                .collect(),
        }
    }

    /// A frame carrying what changed between `self` and `next`, or `None`
    /// when the two screens are identical.
    ///
    /// The result is a full frame when the size changed, because then no row
    /// of the old screen is meaningful any more. A moved cursor or a changed
    /// input mode is a change even when no row differs.
    #[must_use]
    pub fn diff(&self, next: &Self) -> Option<ScreenFrame> {
        if self == next {
            return None;
        }
        if self.size != next.size {
            return Some(next.to_frame());
        }
        Some(ScreenFrame {
            size: next.size,
            cursor: next.cursor,
            modes: next.modes,
            full: false,
            rows: next
                .rows
                .iter()
                .enumerate()
                .filter(|(index, runs)| self.rows.get(*index) != Some(*runs))
                .map(|(index, runs)| ScreenRow {
                    index: u16::try_from(index).unwrap_or(u16::MAX),
                    runs: runs.clone(),
                })
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str) -> StyledRun {
        StyledRun {
            text: text.to_owned(),
            style: CellStyle::default(),
        }
    }

    fn red() -> CellStyle {
        CellStyle {
            fg: Color::Idx(1),
            ..CellStyle::default()
        }
    }

    fn state_with(size: PaneSize, rows: Vec<Vec<StyledRun>>) -> ScreenState {
        ScreenState::from_parts(size, CursorState::default(), InputModes::default(), rows)
    }

    #[test]
    fn a_full_frame_replaces_the_whole_screen() {
        let mut state = ScreenState::new(PaneSize::new(3, 10));
        state.apply(&ScreenFrame {
            size: PaneSize::new(3, 10),
            cursor: CursorState::default(),
            modes: InputModes::default(),
            full: true,
            rows: vec![ScreenRow {
                index: 1,
                runs: vec![run("middle")],
            }],
        });

        assert_eq!(state.row_text(0), "");
        assert_eq!(state.row_text(1), "middle");
        assert_eq!(state.row_text(2), "");
    }

    #[test]
    fn a_partial_frame_keeps_the_rows_it_does_not_mention() {
        let mut state = state_with(
            PaneSize::new(2, 10),
            vec![vec![run("first")], vec![run("second")]],
        );
        state.apply(&ScreenFrame {
            size: PaneSize::new(2, 10),
            cursor: CursorState::default(),
            modes: InputModes::default(),
            full: false,
            rows: vec![ScreenRow {
                index: 1,
                runs: vec![run("changed")],
            }],
        });

        assert_eq!(state.row_text(0), "first");
        assert_eq!(state.row_text(1), "changed");
    }

    #[test]
    fn applying_a_row_blanks_what_the_row_leaves_out() {
        let mut state = state_with(PaneSize::new(1, 10), vec![vec![run("long text")]]);
        state.apply(&ScreenFrame {
            size: PaneSize::new(1, 10),
            cursor: CursorState::default(),
            modes: InputModes::default(),
            full: false,
            rows: vec![ScreenRow {
                index: 0,
                runs: vec![run("hi")],
            }],
        });

        assert_eq!(state.row_text(0), "hi");
    }

    #[test]
    fn rows_outside_the_screen_are_ignored() {
        let mut state = state_with(PaneSize::new(1, 10), vec![vec![run("only")]]);
        state.apply(&ScreenFrame {
            size: PaneSize::new(1, 10),
            cursor: CursorState::default(),
            modes: InputModes::default(),
            full: false,
            rows: vec![ScreenRow {
                index: 9,
                runs: vec![run("nowhere")],
            }],
        });

        assert_eq!(state.row_text(0), "only");
        assert_eq!(state.size(), PaneSize::new(1, 10));
    }

    #[test]
    fn an_unchanged_screen_produces_no_frame() {
        let state = state_with(PaneSize::new(2, 10), vec![vec![run("same")], Vec::new()]);
        assert_eq!(state.diff(&state.clone()), None);
    }

    #[test]
    fn a_diff_carries_only_the_rows_that_changed() {
        let before = state_with(
            PaneSize::new(3, 10),
            vec![vec![run("a")], vec![run("b")], vec![run("c")]],
        );
        let after = state_with(
            PaneSize::new(3, 10),
            vec![vec![run("a")], vec![run("B")], vec![run("c")]],
        );

        let frame = before.diff(&after).expect("the screens differ");
        assert!(!frame.full);
        assert_eq!(frame.rows.len(), 1);
        assert_eq!(frame.rows[0].index, 1);
        assert_eq!(frame.rows[0].runs, vec![run("B")]);
    }

    #[test]
    fn a_diff_reports_a_moved_cursor_without_any_row() {
        let before = state_with(PaneSize::new(2, 10), vec![vec![run("a")], Vec::new()]);
        let mut after = before.clone();
        after.apply(&ScreenFrame {
            size: PaneSize::new(2, 10),
            cursor: CursorState {
                row: 1,
                col: 4,
                visible: true,
                shape: CursorShape::SteadyBar,
            },
            modes: InputModes::default(),
            full: false,
            rows: Vec::new(),
        });

        let frame = before.diff(&after).expect("the cursor moved");
        assert!(frame.rows.is_empty());
        assert_eq!(frame.cursor.row, 1);
        assert_eq!(frame.cursor.col, 4);
        assert!(frame.cursor.visible);
    }

    #[test]
    fn a_diff_reports_a_changed_input_mode_without_any_row() {
        let before = state_with(PaneSize::new(1, 10), vec![vec![run("a")]]);
        let mut after = before.clone();
        after.apply(&ScreenFrame {
            size: PaneSize::new(1, 10),
            cursor: CursorState::default(),
            modes: InputModes {
                application_cursor: true,
                ..InputModes::default()
            },
            full: false,
            rows: Vec::new(),
        });

        let frame = before.diff(&after).expect("a mode changed");
        assert!(frame.rows.is_empty());
        assert!(frame.modes.application_cursor);
    }

    #[test]
    fn a_resize_produces_a_full_frame() {
        let before = state_with(PaneSize::new(2, 10), vec![vec![run("a")], vec![run("b")]]);
        let after = state_with(PaneSize::new(3, 10), vec![vec![run("a")]]);

        let frame = before.diff(&after).expect("the size changed");
        assert!(frame.full);
        assert_eq!(frame.size, PaneSize::new(3, 10));
    }

    #[test]
    fn a_client_that_skips_a_screen_still_ends_up_correct() {
        // What happens when a client is too slow to be sent every screen:
        // it diffs against what it last received, not against what it missed.
        let first = state_with(PaneSize::new(2, 10), vec![vec![run("one")], Vec::new()]);
        let skipped = state_with(PaneSize::new(2, 10), vec![vec![run("two")], Vec::new()]);
        let latest = state_with(
            PaneSize::new(2, 10),
            vec![vec![run("three")], vec![run("new")]],
        );

        let mut client = first.clone();
        assert!(first.diff(&skipped).is_some());
        let frame = first.diff(&latest).expect("the screens differ");
        client.apply(&frame);

        assert_eq!(client, latest);
    }

    #[test]
    fn a_screen_round_trips_through_its_own_full_frame() {
        let state = state_with(
            PaneSize::new(2, 10),
            vec![
                vec![
                    run("ab"),
                    StyledRun {
                        text: "cd".to_owned(),
                        style: red(),
                    },
                ],
                Vec::new(),
            ],
        );

        let mut rebuilt = ScreenState::new(PaneSize::new(2, 10));
        rebuilt.apply(&state.to_frame());
        assert_eq!(rebuilt, state);
    }

    #[test]
    fn plain_text_leaves_out_trailing_blanks() {
        let state = state_with(PaneSize::new(2, 10), vec![vec![run("hi    ")], Vec::new()]);
        assert_eq!(state.row_text(0), "hi");
        assert_eq!(state.text(), "hi\n");
    }

    #[test]
    fn missing_rows_are_blank_and_extra_rows_are_dropped() {
        let short = state_with(PaneSize::new(3, 10), vec![vec![run("a")]]);
        assert_eq!(short.rows().count(), 3);

        let long = state_with(
            PaneSize::new(1, 10),
            vec![vec![run("a")], vec![run("b")], vec![run("c")]],
        );
        assert_eq!(long.rows().count(), 1);
        assert_eq!(long.row_text(0), "a");
    }

    #[test]
    fn unstyled_cells_and_inactive_modes_stay_out_of_the_encoded_frame() {
        let frame = ScreenFrame {
            size: PaneSize::new(1, 4),
            cursor: CursorState::default(),
            modes: InputModes::default(),
            full: true,
            rows: vec![ScreenRow {
                index: 0,
                runs: vec![run("hi")],
            }],
        };

        let line = serde_json::to_string(&frame).expect("a frame encodes");
        assert!(!line.contains("bold"), "{line}");
        assert!(!line.contains("application_cursor"), "{line}");
        assert!(!line.contains("mouse"), "{line}");
        assert!(!line.contains("\"style\""), "{line}");

        let decoded: ScreenFrame = serde_json::from_str(&line).expect("a frame decodes");
        assert_eq!(decoded, frame);
    }

    #[test]
    fn styles_and_modes_round_trip() {
        let frame = ScreenFrame {
            size: PaneSize::new(1, 4),
            cursor: CursorState {
                row: 0,
                col: 2,
                visible: true,
                shape: CursorShape::BlinkUnderline,
            },
            modes: InputModes {
                application_cursor: true,
                application_keypad: true,
                bracketed_paste: true,
                alternate_screen: true,
                mouse: MouseMode::ButtonMotion,
                mouse_encoding: MouseEncoding::Sgr,
            },
            full: false,
            rows: vec![ScreenRow {
                index: 0,
                runs: vec![StyledRun {
                    text: "x".to_owned(),
                    style: CellStyle {
                        fg: Color::Rgb(1, 2, 3),
                        bg: Color::Idx(9),
                        bold: true,
                        dim: true,
                        italic: true,
                        underline: true,
                        inverse: true,
                    },
                }],
            }],
        };

        let line = serde_json::to_string(&frame).expect("a frame encodes");
        let decoded: ScreenFrame = serde_json::from_str(&line).expect("a frame decodes");
        assert_eq!(decoded, frame);
    }

    #[test]
    fn unknown_enum_values_decode_to_unknown_instead_of_failing() {
        let modes: InputModes =
            serde_json::from_str(r#"{"mouse":"telepathy","mouse_encoding":"runes"}"#)
                .expect("modes decode");
        assert_eq!(modes.mouse, MouseMode::Unknown);
        assert_eq!(modes.mouse_encoding, MouseEncoding::Unknown);
        assert!(modes.mouse.is_on());

        let cursor: CursorState =
            serde_json::from_str(r#"{"row":0,"col":0,"visible":true,"shape":"hourglass"}"#)
                .expect("a cursor decodes");
        assert_eq!(cursor.shape, CursorShape::Unknown);
    }

    #[test]
    fn a_frame_with_no_rows_at_all_decodes() {
        let frame: ScreenFrame = serde_json::from_str(
            r#"{"size":{"rows":2,"cols":4},"cursor":{"row":0,"col":0,"visible":false}}"#,
        )
        .expect("a frame decodes");
        assert!(frame.rows.is_empty());
        assert!(!frame.full);
    }
}
