//! Taking over the user's terminal, and giving it back.
//!
//! A pane's program needs the terminal raw: every key has to arrive as bytes
//! rather than as lines the terminal has already interpreted. That change
//! must be undone whatever happens, including on a panic — a terminal left
//! raw and on the alternate screen looks broken and takes a `reset` to fix.

use std::io::{self, IsTerminal, Stdout, Write};

use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

/// The terminal otag draws into.
pub type Tui = Terminal<CrosstermBackend<Stdout>>;

/// Holds the terminal's changed state, and restores it when dropped.
///
/// Dropping this is the only way the terminal is restored, so it must live
/// for as long as otag is drawing.
#[derive(Debug)]
pub struct Guard {
    restored: bool,
}

impl Guard {
    /// Put the terminal into the state a pane's program needs.
    ///
    /// Fails when otag is not running in a terminal, because then there is
    /// nothing to draw into and no keys to read.
    pub fn enter() -> io::Result<Self> {
        if !io::stdout().is_terminal() {
            return Err(io::Error::other(
                "otag needs a terminal to draw into. \
                 Run it from a terminal, not with its output redirected.",
            ));
        }

        enable_raw_mode()?;
        let mut stdout = io::stdout();
        // Drawing on the alternate screen keeps whatever the user had in
        // their terminal: leaving puts it back untouched.
        crossterm::execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;

        Ok(Self { restored: false })
    }

    /// Give the terminal back. Called automatically when the guard is
    /// dropped; calling it earlier makes a failure visible instead of
    /// silent.
    pub fn restore(&mut self) -> io::Result<()> {
        if self.restored {
            return Ok(());
        }
        self.restored = true;

        let mut stdout = io::stdout();
        let left = crossterm::execute!(stdout, DisableBracketedPaste, LeaveAlternateScreen);
        let raw = disable_raw_mode();
        let flushed = stdout.flush();
        left.and(raw).and(flushed)
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Err(error) = self.restore() {
            // Nothing useful can be reported here: the terminal is the only
            // place a message could go, and it is in an unknown state.
            tracing::debug!(%error, "cannot restore the terminal");
        }
    }
}

/// Restore the terminal before a panic is printed.
///
/// Without this, a panic message is written to a raw alternate screen that
/// is then torn down, so the user sees a broken terminal and no explanation.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let mut stdout = io::stdout();
        let _ = crossterm::execute!(stdout, DisableBracketedPaste, LeaveAlternateScreen);
        let _ = disable_raw_mode();
        let _ = stdout.flush();
        previous(info);
    }));
}

/// A terminal to draw into, with the screen cleared.
pub fn start() -> io::Result<Tui> {
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    terminal.clear()?;
    Ok(terminal)
}
