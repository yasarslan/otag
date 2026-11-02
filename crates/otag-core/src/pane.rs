//! Identity, size and spawn description of a pane.
//!
//! A pane is one pseudo-terminal with one child process on it. This module
//! holds the plain data describing a pane; [`crate::pty`] runs it and
//! [`crate::screen`] keeps what it has drawn.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::num::NonZeroU16;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Environment variable carrying the pane's identifier to its child process.
pub const PANE_ID_ENV: &str = "OTAG_PANE_ID";

/// Environment variable carrying the pane's workspace identifier.
pub const WORKSPACE_ID_ENV: &str = "OTAG_WORKSPACE_ID";

/// Terminal type otag's screen emulation implements.
///
/// The emulator handles the xterm repertoire otag needs: alternate screen,
/// scroll regions, insert and delete, indexed and RGB colors, bracketed paste
/// and SGR mouse reporting.
pub const TERM: &str = "xterm-256color";

/// Advertised to children that look at `COLORTERM` before emitting 24-bit
/// color. The emulator keeps RGB colors as RGB, and the client renders them
/// as RGB, so the claim holds end to end.
pub const COLORTERM: &str = "truecolor";

/// Identifier of a pane, unique within one daemon run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PaneId(u64);

impl PaneId {
    /// Identifier handed to the first pane a daemon creates.
    pub const FIRST: Self = Self(1);

    /// The identifier after this one.
    ///
    /// Saturates instead of wrapping, so identifiers are never reused; a
    /// daemon would have to create panes for longer than the universe has
    /// existed to reach the limit.
    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }

    /// The identifier as a number.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for PaneId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// Size of a pane in character cells.
///
/// A size is always at least one row by one column and never larger than
/// [`PaneSize::MAX_ROWS`] by [`PaneSize::MAX_COLS`]: the values arrive from a
/// client's terminal, and neither an empty nor an absurd grid may be
/// allocated. Values outside the range are clamped on construction, including
/// when a size is deserialized, so an invalid one cannot be built at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "RawPaneSize")]
pub struct PaneSize {
    rows: u16,
    cols: u16,
}

/// The wire shape of [`PaneSize`], before clamping.
///
/// The fields are wider than a [`PaneSize`]'s so that a number far outside
/// the supported range is clamped like any other rather than failing to
/// decode at all.
#[derive(Deserialize)]
struct RawPaneSize {
    #[serde(default)]
    rows: u32,
    #[serde(default)]
    cols: u32,
}

impl From<RawPaneSize> for PaneSize {
    fn from(raw: RawPaneSize) -> Self {
        Self::new(saturate(raw.rows), saturate(raw.cols))
    }
}

/// Narrow a wire value to the widest size otag could ever use.
const fn saturate(value: u32) -> u16 {
    if value > u16::MAX as u32 {
        u16::MAX
    } else {
        value as u16
    }
}

impl PaneSize {
    /// Largest number of rows otag allocates for a pane.
    pub const MAX_ROWS: u16 = 1024;

    /// Largest number of columns otag allocates for a pane.
    pub const MAX_COLS: u16 = 1024;

    /// The classic terminal size, used when the real one is not known yet.
    pub const DEFAULT: Self = Self { rows: 24, cols: 80 };

    /// A size clamped into the supported range.
    #[must_use]
    pub const fn new(rows: u16, cols: u16) -> Self {
        Self {
            rows: clamp(rows, Self::MAX_ROWS),
            cols: clamp(cols, Self::MAX_COLS),
        }
    }

    /// Number of rows.
    #[must_use]
    pub const fn rows(self) -> u16 {
        self.rows
    }

    /// Number of columns.
    #[must_use]
    pub const fn cols(self) -> u16 {
        self.cols
    }

    /// Number of rows, for interfaces that require a non-zero value.
    #[must_use]
    pub const fn rows_nonzero(self) -> NonZeroU16 {
        match NonZeroU16::new(self.rows) {
            Some(rows) => rows,
            // Unreachable: `new` clamps to at least one.
            None => NonZeroU16::MIN,
        }
    }

    /// Number of columns, for interfaces that require a non-zero value.
    #[must_use]
    pub const fn cols_nonzero(self) -> NonZeroU16 {
        match NonZeroU16::new(self.cols) {
            Some(cols) => cols,
            None => NonZeroU16::MIN,
        }
    }
}

/// `Ord::clamp` is not usable in a constant function yet.
const fn clamp(value: u16, max: u16) -> u16 {
    if value < 1 {
        1
    } else if value > max {
        max
    } else {
        value
    }
}

impl Default for PaneSize {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl fmt::Display for PaneSize {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}x{}", self.cols, self.rows)
    }
}

/// What to run on a pane's pseudo-terminal, and in which environment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PaneSpec {
    /// Program and arguments. Empty means the user's login shell.
    command: Vec<OsString>,
    /// Working directory. `None` means the daemon's own.
    cwd: Option<PathBuf>,
    /// Variables to set on top of the inherited environment.
    env: Vec<(OsString, OsString)>,
}

impl PaneSpec {
    /// A pane running the user's shell.
    #[must_use]
    pub fn shell() -> Self {
        Self::default()
    }

    /// A pane running `command`, which must not be empty to take effect.
    #[must_use]
    pub fn command<I, S>(command: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        Self {
            command: command
                .into_iter()
                .map(|part| part.as_ref().to_owned())
                .collect(),
            ..Self::default()
        }
    }

    /// Run in `cwd`.
    #[must_use]
    pub fn with_cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    /// Run in `cwd`, if one is given.
    #[must_use]
    pub fn with_cwd_opt(mut self, cwd: Option<impl Into<PathBuf>>) -> Self {
        self.cwd = cwd.map(Into::into);
        self
    }

    /// Set an environment variable for the child.
    #[must_use]
    pub fn with_env(mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> Self {
        self.env
            .push((key.as_ref().to_owned(), value.as_ref().to_owned()));
        self
    }

    /// Program and arguments, empty when the user's shell should be run.
    #[must_use]
    pub fn argv(&self) -> &[OsString] {
        &self.command
    }

    /// Working directory, if one was chosen.
    #[must_use]
    pub fn cwd(&self) -> Option<&Path> {
        self.cwd.as_deref()
    }

    /// Variables to set on top of the inherited environment.
    #[must_use]
    pub fn env(&self) -> &[(OsString, OsString)] {
        &self.env
    }
}

/// How a pane's child process ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneExit {
    /// Exit code, when the process exited normally.
    pub code: Option<u32>,
    /// Name of the signal that killed the process, when it was signalled.
    pub signal: Option<String>,
}

impl PaneExit {
    /// Whether the process reported success.
    #[must_use]
    pub fn success(&self) -> bool {
        self.signal.is_none() && self.code == Some(0)
    }
}

impl fmt::Display for PaneExit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.signal, self.code) {
            (Some(signal), _) => write!(formatter, "killed by {signal}"),
            (None, Some(code)) => write!(formatter, "exited with status {code}"),
            (None, None) => formatter.write_str("exited"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_ids_increase_and_never_wrap() {
        assert_eq!(PaneId::FIRST.get(), 1);
        assert_eq!(PaneId::FIRST.next().get(), 2);
        assert_eq!(PaneId::FIRST.to_string(), "1");
    }

    #[test]
    fn sizes_are_clamped_into_the_supported_range() {
        assert_eq!(PaneSize::new(0, 0), PaneSize::new(1, 1));
        assert_eq!(PaneSize::new(5, 9).rows(), 5);
        assert_eq!(PaneSize::new(5, 9).cols(), 9);
        let huge = PaneSize::new(u16::MAX, u16::MAX);
        assert_eq!(huge.rows(), PaneSize::MAX_ROWS);
        assert_eq!(huge.cols(), PaneSize::MAX_COLS);
    }

    #[test]
    fn sizes_are_never_zero_even_when_deserialized() {
        let decoded: PaneSize =
            serde_json::from_str(r#"{"rows":0,"cols":0}"#).expect("a size decodes");
        assert_eq!(decoded, PaneSize::new(1, 1));
        assert_eq!(decoded.rows_nonzero().get(), 1);
        assert_eq!(decoded.cols_nonzero().get(), 1);

        let missing: PaneSize = serde_json::from_str("{}").expect("an empty size decodes");
        assert_eq!(missing, PaneSize::new(1, 1));

        // A number too large for a terminal, or even for the field, is
        // clamped rather than rejected.
        let over: PaneSize =
            serde_json::from_str(r#"{"rows":99999,"cols":4000000000}"#).expect("a size decodes");
        assert_eq!(over.rows(), PaneSize::MAX_ROWS);
        assert_eq!(over.cols(), PaneSize::MAX_COLS);
    }

    #[test]
    fn sizes_roundtrip_through_json() {
        let size = PaneSize::new(30, 100);
        let line = serde_json::to_string(&size).expect("a size encodes");
        assert_eq!(line, r#"{"rows":30,"cols":100}"#);
        let decoded: PaneSize = serde_json::from_str(&line).expect("a size decodes");
        assert_eq!(decoded, size);
    }

    #[test]
    fn sizes_display_as_columns_by_rows() {
        assert_eq!(PaneSize::DEFAULT.to_string(), "80x24");
    }

    #[test]
    fn specs_describe_a_shell_or_a_command() {
        assert!(PaneSpec::shell().argv().is_empty());

        let spec = PaneSpec::command(["echo", "hi"])
            .with_cwd("/tmp")
            .with_env("A", "b");
        assert_eq!(spec.argv(), ["echo", "hi"].map(OsString::from));
        assert_eq!(spec.cwd(), Some(Path::new("/tmp")));
        assert_eq!(spec.env(), [(OsString::from("A"), OsString::from("b"))]);

        assert_eq!(PaneSpec::shell().with_cwd_opt(None::<PathBuf>).cwd(), None);
    }

    #[test]
    fn exits_report_success_and_read_clearly() {
        let ok = PaneExit {
            code: Some(0),
            signal: None,
        };
        assert!(ok.success());
        assert_eq!(ok.to_string(), "exited with status 0");

        let failed = PaneExit {
            code: Some(1),
            signal: None,
        };
        assert!(!failed.success());

        let killed = PaneExit {
            code: None,
            signal: Some("SIGTERM".to_owned()),
        };
        assert!(!killed.success());
        assert_eq!(killed.to_string(), "killed by SIGTERM");
    }
}
