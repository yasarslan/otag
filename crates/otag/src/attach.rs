//! Running otag's terminal interface against an attached pane.
//!
//! The client draws what the daemon sends and sends what the user does.
//! It keeps no state about the pane beyond the screen it has been given, so
//! closing it — or losing it — leaves the pane and its program untouched.

use std::path::Path;

use anyhow::{Context, Result, bail};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use otag_core::client::{self, Reader, Writer};
use otag_core::frame::ScreenState;
use otag_core::pane::{PaneExit, PaneSize};
use otag_core::proto::{Request, Response};
use otag_ui::terminal::{Guard, Tui};
use otag_ui::{keys, render};
use tokio::sync::mpsc;

/// The key that introduces an otag command instead of going to the pane.
const PREFIX: KeyEvent = KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL);

/// How many input events may wait while the client is busy drawing.
const INPUT_QUEUE: usize = 256;

/// Attach to a pane and draw it until the user detaches or it ends.
pub async fn run(socket: &Path) -> Result<()> {
    // Checked before anything else: a client that cannot draw should not
    // start a daemon, and the failure should reach the user plainly rather
    // than from inside a terminal that has already been taken over.
    otag_ui::terminal::require().context("cannot attach to a pane")?;

    crate::client::ensure_daemon(socket).await?;

    otag_ui::terminal::install_panic_hook();
    let mut guard = Guard::enter().context("cannot prepare the terminal for otag")?;
    let outcome = attached(socket).await;
    guard
        .restore()
        .context("cannot restore the terminal after otag")?;

    match outcome? {
        Outcome::Detached => Ok(()),
        Outcome::Exited(exit) => {
            // A program that ended cleanly needs no comment; one that did
            // not is worth a line, since its output may have scrolled away.
            if !exit.success() {
                eprintln!("otag: the pane's program {exit}");
            }
            Ok(())
        }
    }
}

/// Why the client stopped drawing.
enum Outcome {
    /// The user asked to detach. The pane keeps running.
    Detached,
    /// The pane's program ended.
    Exited(PaneExit),
}

/// Attach and run the event loop with the terminal already taken over.
async fn attached(socket: &Path) -> Result<Outcome> {
    let mut terminal = otag_ui::terminal::start().context("cannot start drawing")?;
    let size = terminal_size(&terminal)?;

    let mut connection = client::Connection::connect(socket).await?;
    let response = connection
        .request(&Request::Attach {
            size,
            cwd: std::env::current_dir().ok(),
        })
        .await
        .context("cannot attach to a pane")?;

    let (pane, frame) = match response {
        Response::Attached { pane, frame } => (pane, frame),
        Response::Error { message } => bail!("{message}"),
        other => bail!("the otag daemon sent an unexpected answer: {other:?}"),
    };
    tracing::debug!(%pane, "attached to a pane");

    let mut screen = ScreenState::new(size);
    screen.apply(&frame);

    let (reader, writer) = connection.into_split();
    let input = read_input();
    draw(&mut terminal, &screen)?;

    event_loop(&mut terminal, &mut screen, reader, writer, input).await
}

/// Draw frames, send keys, and keep the pane's size matched to the terminal.
async fn event_loop(
    terminal: &mut Tui,
    screen: &mut ScreenState,
    mut reader: Reader,
    mut writer: Writer,
    mut input: mpsc::Receiver<Event>,
) -> Result<Outcome> {
    let mut prefix_held = false;

    loop {
        tokio::select! {
            message = reader.receive() => match message? {
                Response::Frame { frame, .. } => {
                    screen.apply(&frame);
                    draw(terminal, screen)?;
                }
                Response::PaneExited { exit, .. } => return Ok(Outcome::Exited(exit)),
                Response::Error { message } => bail!("{message}"),
                // A daemon of another version may send something this build
                // does not know; ignoring it is better than giving up.
                other => tracing::debug!(?other, "ignoring a message otag does not know"),
            },

            event = input.recv() => {
                let Some(event) = event else {
                    // Reading the terminal failed, so there is no way left
                    // to control the pane.
                    return Ok(Outcome::Detached);
                };
                match event {
                    Event::Key(key) if key.kind != KeyEventKind::Release => {
                        match command(key, &mut prefix_held) {
                            Command::Detach => {
                                // Ask the daemon to let go, so the pane is
                                // left in a known state rather than simply
                                // losing its client.
                                let _ = writer.send(&Request::Detach).await;
                                return Ok(Outcome::Detached);
                            }
                            Command::Send(key) => {
                                if let Some(bytes) = keys::encode(key, screen.modes()) {
                                    writer.send(&Request::Input { bytes }).await?;
                                }
                            }
                            Command::Ignore => {}
                        }
                    }
                    Event::Paste(text) => {
                        let bytes = keys::encode_paste(&text, screen.modes());
                        writer.send(&Request::Input { bytes }).await?;
                    }
                    Event::Resize(cols, rows) => {
                        let size = PaneSize::new(rows, cols);
                        writer.send(&Request::Resize { size }).await?;
                        // The pane answers with a full frame; until it
                        // arrives, draw what there is at the new size.
                        draw(terminal, screen)?;
                    }
                    // Key releases, focus changes and mouse events are not
                    // forwarded yet.
                    _ => {}
                }
            }
        }
    }
}

/// What a key press means.
enum Command {
    /// Detach from the pane.
    Detach,
    /// Send this key to the pane.
    Send(KeyEvent),
    /// Do nothing.
    Ignore,
}

/// Decide whether a key is an otag command or belongs to the pane.
///
/// The prefix key is how otag's own commands stay out of the way of the
/// programs in a pane, which use nearly every key themselves. Pressing it
/// twice sends it on, so a program that wants the prefix key can still have
/// it.
fn command(key: KeyEvent, prefix_held: &mut bool) -> Command {
    if *prefix_held {
        *prefix_held = false;
        return match key.code {
            KeyCode::Char('d') if key.modifiers == KeyModifiers::NONE => Command::Detach,
            _ if same_key(key, PREFIX) => Command::Send(PREFIX),
            // An unknown command does nothing, rather than reaching the pane
            // as a key the user did not mean to press.
            _ => Command::Ignore,
        };
    }

    if same_key(key, PREFIX) {
        *prefix_held = true;
        return Command::Ignore;
    }
    Command::Send(key)
}

/// Whether two key presses are the same key with the same modifiers.
fn same_key(left: KeyEvent, right: KeyEvent) -> bool {
    left.code == right.code && left.modifiers == right.modifiers
}

/// Draw the pane, with the cursor where the pane has it.
fn draw(terminal: &mut Tui, screen: &ScreenState) -> Result<()> {
    terminal
        .draw(|frame| {
            let area = frame.area();
            let pane = render::Pane::new(screen);
            frame.render_widget(pane, area);
            if let Some(position) = pane.cursor_position(area) {
                frame.set_cursor_position(position);
            }
        })
        .context("cannot draw the pane")?;
    Ok(())
}

/// The size of the terminal otag is drawing into.
fn terminal_size(terminal: &Tui) -> Result<PaneSize> {
    let size = terminal.size().context("cannot read the terminal's size")?;
    Ok(PaneSize::new(size.height, size.width))
}

/// Read the terminal's events on a thread of their own.
///
/// Reading the terminal blocks, so it cannot share a task with the socket.
/// The thread ends with the process; there is nothing to clean up, because
/// the terminal is restored by its guard.
fn read_input() -> mpsc::Receiver<Event> {
    let (events_tx, events_rx) = mpsc::channel(INPUT_QUEUE);

    if let Err(error) = std::thread::Builder::new()
        .name("otag-input".to_owned())
        .spawn(move || {
            loop {
                match crossterm::event::read() {
                    Ok(event) => {
                        if events_tx.blocking_send(event).is_err() {
                            return;
                        }
                    }
                    Err(error) => {
                        tracing::warn!(%error, "cannot read the terminal");
                        return;
                    }
                }
            }
        })
    {
        tracing::error!(%error, "cannot start the thread reading the terminal");
    }

    events_rx
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn sent(command: Command) -> KeyEvent {
        match command {
            Command::Send(key) => key,
            Command::Detach => panic!("expected a key, got a detach"),
            Command::Ignore => panic!("expected a key, got nothing"),
        }
    }

    #[test]
    fn ordinary_keys_go_straight_to_the_pane() {
        let mut prefix_held = false;
        assert_eq!(
            sent(command(key(KeyCode::Char('a')), &mut prefix_held)),
            key(KeyCode::Char('a'))
        );
        assert!(!prefix_held);
    }

    #[test]
    fn the_prefix_key_is_held_back_rather_than_sent() {
        let mut prefix_held = false;
        assert!(matches!(command(PREFIX, &mut prefix_held), Command::Ignore));
        assert!(prefix_held);
    }

    #[test]
    fn the_prefix_followed_by_d_detaches() {
        let mut prefix_held = true;
        assert!(matches!(
            command(key(KeyCode::Char('d')), &mut prefix_held),
            Command::Detach
        ));
        assert!(!prefix_held, "the prefix is spent either way");
    }

    #[test]
    fn the_prefix_twice_sends_the_prefix_key_itself() {
        // A program in the pane that uses this key can still receive it.
        let mut prefix_held = true;
        assert_eq!(sent(command(PREFIX, &mut prefix_held)), PREFIX);
        assert!(!prefix_held);
    }

    #[test]
    fn an_unknown_command_after_the_prefix_does_nothing() {
        // Reaching the pane as a key the user did not mean to press would be
        // worse than doing nothing.
        let mut prefix_held = true;
        assert!(matches!(
            command(key(KeyCode::Char('q')), &mut prefix_held),
            Command::Ignore
        ));
        assert!(!prefix_held);
    }

    #[test]
    fn the_detach_key_reaches_the_pane_when_the_prefix_was_not_pressed() {
        let mut prefix_held = false;
        assert_eq!(
            sent(command(key(KeyCode::Char('d')), &mut prefix_held)),
            key(KeyCode::Char('d'))
        );
    }

    #[test]
    fn the_detach_key_with_a_modifier_is_not_a_detach() {
        let mut prefix_held = true;
        let modified = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL);
        assert!(matches!(
            command(modified, &mut prefix_held),
            Command::Ignore
        ));
    }
}
