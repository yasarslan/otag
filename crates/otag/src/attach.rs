//! Running otag's terminal interface against an attached session.
//!
//! The client draws what the daemon sends and sends what the user does. It
//! keeps no state about the session beyond the view it has been given, so
//! closing it — or losing it — leaves the panes and their programs
//! untouched.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind,
};
use otag_core::client::{self, Reader, Writer};
use otag_core::frame::ScreenState;
use otag_core::layout::{BORDER, Direction, Side};
use otag_core::pane::{PaneExit, PaneSize};
use otag_core::proto::{Request, Response, SessionView};
use otag_ui::session::Screens;
use otag_ui::terminal::{Guard, Tui};
use otag_ui::theme::Theme;
use otag_ui::{keys, session};
use tokio::sync::mpsc;

/// The key that introduces an otag command instead of going to the pane.
const PREFIX: KeyEvent = KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL);

/// How many input events may wait while the client is busy drawing.
const INPUT_QUEUE: usize = 256;

/// How far one resize key press moves a pane's edge.
const RESIZE_CELLS: u16 = 3;

/// How often the client compares the terminal's size with the session's.
///
/// A terminal announces a resize with a signal, and a signal can go missing:
/// it is not queued, and a client that is not the foreground process group of
/// its terminal never receives one at all. Panes left at the wrong size draw
/// into part of the terminal and tell their programs the wrong thing, which
/// the user can only fix by resizing again. Comparing the sizes on a slow
/// timer costs one query of the terminal and makes a missed signal heal
/// itself instead.
const SIZE_CHECK: Duration = Duration::from_secs(1);

/// Attach to the session and draw it until the user detaches or it ends.
pub async fn run(socket: &Path) -> Result<()> {
    // Checked before anything else: a client that cannot draw should not
    // start a daemon, and the failure should reach the user plainly rather
    // than from inside a terminal that has already been taken over.
    otag_ui::terminal::require().context("cannot attach to a session")?;

    crate::client::ensure_daemon(socket).await?;

    otag_ui::terminal::install_panic_hook();
    let mut guard = Guard::enter().context("cannot prepare the terminal for otag")?;
    let outcome = attached(socket).await;
    guard
        .restore()
        .context("cannot restore the terminal after otag")?;

    match outcome? {
        Outcome::Detached => Ok(()),
        Outcome::Ended(exit) => {
            // A program that ended cleanly needs no comment; one that did
            // not is worth a line, since its output may have scrolled away.
            if let Some(exit) = exit.filter(|exit| !exit.success()) {
                eprintln!("otag: the pane's program {exit}");
            }
            Ok(())
        }
    }
}

/// Why the client stopped drawing.
enum Outcome {
    /// The user asked to detach. The panes keep running.
    Detached,
    /// The session has no panes left.
    Ended(Option<PaneExit>),
}

/// What the client is drawing, and what it has been told.
struct Client {
    view: SessionView,
    screens: Screens,
    theme: Theme,
}

impl Client {
    /// A client with nothing drawn yet.
    fn new(size: PaneSize) -> Self {
        Self {
            view: SessionView {
                size,
                ..SessionView::default()
            },
            screens: Screens::new(),
            theme: Theme::default(),
        }
    }

    /// Take a whole view, which carries every visible pane's screen.
    fn apply_view(&mut self, view: SessionView) {
        let mut screens = Screens::with_capacity(view.panes.len());
        for pane in &view.panes {
            let mut screen = ScreenState::new(pane.area.inset(BORDER).size());
            if let Some(frame) = &pane.frame {
                screen.apply(frame);
            } else if let Some(previous) = self.screens.remove(&pane.id) {
                // A view without a screen for a pane says nothing about it,
                // so what the client already had is still the best picture.
                screen = previous;
            }
            screens.insert(pane.id, screen);
        }
        self.screens = screens;
        self.view = view;
    }

    /// Take one pane's update.
    fn apply_frame(
        &mut self,
        pane: otag_core::pane::PaneId,
        frame: &otag_core::frame::ScreenFrame,
    ) {
        self.screens
            .entry(pane)
            .or_insert_with(|| ScreenState::new(frame.size))
            .apply(frame);
    }

    /// Draw everything.
    fn draw(&self, terminal: &mut Tui) -> Result<()> {
        terminal
            .draw(|frame| session::draw(frame, &self.view, &self.screens, self.theme))
            .context("cannot draw the session")?;
        Ok(())
    }
}

/// Attach and run the event loop with the terminal already taken over.
async fn attached(socket: &Path) -> Result<Outcome> {
    let mut terminal = otag_ui::terminal::start().context("cannot start drawing")?;
    let size = terminal_size(&terminal)?;
    let cwd = std::env::current_dir().ok();

    let mut connection = client::Connection::connect(socket).await?;
    let response = connection
        .request(&Request::Attach {
            size,
            cwd: cwd.clone(),
        })
        .await
        .context("cannot attach to the session")?;

    let view = match response {
        Response::View { view } => *view,
        Response::Error { message } => bail!("{message}"),
        other => bail!("the otag daemon sent an unexpected answer: {other:?}"),
    };
    tracing::debug!(panes = view.panes.len(), "attached to a session");

    let mut client = Client::new(size);
    client.apply_view(view);
    client.draw(&mut terminal)?;

    let (reader, writer) = connection.into_split();
    let input = read_input();

    event_loop(&mut terminal, &mut client, size, cwd, reader, writer, input).await
}

/// Draw what arrives, send what the user does, and keep the session's size
/// matched to the terminal.
async fn event_loop(
    terminal: &mut Tui,
    client: &mut Client,
    size: PaneSize,
    cwd: Option<PathBuf>,
    mut reader: Reader,
    mut writer: Writer,
    mut input: mpsc::Receiver<Event>,
) -> Result<Outcome> {
    let mut prefix_held = false;
    // The size the daemon was last told about, so that a resize is sent once
    // however it was noticed.
    let mut sent_size = size;
    let mut size_check = tokio::time::interval(SIZE_CHECK);
    size_check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            message = reader.receive() => match message {
                Ok(Response::View { view }) => {
                    client.apply_view(*view);
                    client.draw(terminal)?;
                }
                Ok(Response::Frame { pane, frame }) => {
                    client.apply_frame(pane, &frame);
                    client.draw(terminal)?;
                }
                Ok(Response::SessionEnded { exit }) => return Ok(Outcome::Ended(exit)),
                Ok(Response::Error { message }) => bail!("{message}"),
                // A daemon of another version may send something this build
                // does not know; ignoring it is better than giving up.
                Ok(other) => tracing::debug!(?other, "ignoring a message otag does not know"),
                // The daemon has gone, which is what `otag kill-server` from
                // another terminal looks like from here: there is nothing
                // left to draw, and nothing the user did wrong.
                Err(otag_core::Error::ConnectionClosed) => return Ok(Outcome::Ended(None)),
                Err(error) => return Err(error.into()),
            },

            event = input.recv() => {
                let Some(event) = event else {
                    // Reading the terminal failed, so there is no way left
                    // to control the session.
                    return Ok(Outcome::Detached);
                };
                let command = match event {
                    Event::Key(key) if key.kind != KeyEventKind::Release => {
                        command(key, &mut prefix_held, &client.view)
                    }
                    Event::Mouse(mouse) => {
                        mouse_command(mouse, &client.view, &client.screens)
                    }
                    Event::Paste(text) => Command::Ask(Request::Input {
                        bytes: keys::encode_paste(&text, focused_modes(client)),
                    }),
                    Event::Resize(cols, rows) => {
                        let size = PaneSize::new(rows, cols);
                        if size == sent_size {
                            Command::Ignore
                        } else {
                            sent_size = size;
                            Command::Ask(Request::Resize { size })
                        }
                    }
                    // Key releases and focus changes are not forwarded.
                    _ => Command::Ignore,
                };

                match command {
                    Command::Detach => {
                        // Ask the daemon to let go, so the session is left
                        // in a known state rather than simply losing its
                        // client.
                        let _ = writer.send(&Request::Detach).await;
                        return Ok(Outcome::Detached);
                    }
                    Command::Send(key) => {
                        if let Some(bytes) = keys::encode(key, focused_modes(client)) {
                            writer.send(&Request::Input { bytes }).await?;
                        }
                    }
                    Command::NewWorkspace => {
                        writer.send(&Request::NewWorkspace { cwd: cwd.clone() }).await?;
                    }
                    Command::Ask(request) => writer.send(&request).await?,
                    Command::Ignore => {}
                }
            }

            _ = size_check.tick() => {
                match terminal_size(terminal) {
                    Ok(size) if size != sent_size => {
                        tracing::debug!(?sent_size, ?size, "the terminal changed size without saying so");
                        sent_size = size;
                        writer.send(&Request::Resize { size }).await?;
                    }
                    Ok(_) => {}
                    // Without a size there is nothing to compare; the next
                    // tick tries again rather than ending the session.
                    Err(error) => tracing::debug!(%error, "cannot read the terminal's size"),
                }
            }
        }
    }
}

/// The input modes of the pane the user is typing into.
///
/// What a key turns into depends on them, so a key is encoded for the pane it
/// is going to and not for whichever pane the client drew last.
fn focused_modes(client: &Client) -> otag_core::frame::InputModes {
    client
        .view
        .focus
        .and_then(|pane| client.screens.get(&pane))
        .map(ScreenState::modes)
        .unwrap_or_default()
}

/// What something the user did means.
enum Command {
    /// Detach from the session.
    Detach,
    /// Send this key to the focused pane.
    Send(KeyEvent),
    /// Send this request to the daemon.
    Ask(Request),
    /// Add a workspace for the directory the client was started in.
    NewWorkspace,
    /// Do nothing.
    Ignore,
}

/// Decide whether a key is an otag command or belongs to the focused pane.
///
/// The prefix key is how otag's own commands stay out of the way of the
/// programs in a pane, which use nearly every key themselves. Pressing it
/// twice sends it on, so a program that wants the prefix key can still have
/// it.
fn command(key: KeyEvent, prefix_held: &mut bool, view: &SessionView) -> Command {
    if *prefix_held {
        *prefix_held = false;
        return prefixed(key, view);
    }

    if same_key(key, PREFIX) {
        *prefix_held = true;
        return Command::Ignore;
    }
    Command::Send(key)
}

/// What a key means when the prefix was pressed first.
fn prefixed(key: KeyEvent, view: &SessionView) -> Command {
    // Shift is part of what a letter is, so only control and alt disqualify
    // a key from being a command.
    if key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        && !same_key(key, PREFIX)
    {
        return Command::Ignore;
    }

    match key.code {
        KeyCode::Char('d') => Command::Detach,
        KeyCode::Char('%') => Command::Ask(Request::Split {
            direction: Direction::LeftRight,
        }),
        KeyCode::Char('"') => Command::Ask(Request::Split {
            direction: Direction::TopBottom,
        }),
        KeyCode::Char('c') => Command::Ask(Request::NewTab),
        KeyCode::Char('n') => step_tab(view, 1),
        KeyCode::Char('p') => step_tab(view, -1),
        KeyCode::Char(digit @ '1'..='9') => {
            // Tabs are numbered from one on screen, from zero underneath.
            let index = usize::from(u32::from(digit) as u8 - b'1');
            if index < view.tabs.len() {
                Command::Ask(Request::SelectTab { index })
            } else {
                Command::Ignore
            }
        }
        KeyCode::Char('w') => Command::NewWorkspace,
        KeyCode::Char(')') => step_workspace(view, 1),
        KeyCode::Char('(') => step_workspace(view, -1),
        KeyCode::Char('o') | KeyCode::Tab => Command::Ask(Request::FocusNext),
        KeyCode::Char('z') => Command::Ask(Request::ToggleSidebar),

        KeyCode::Char('h') | KeyCode::Left => focus(Side::Left),
        KeyCode::Char('j') | KeyCode::Down => focus(Side::Down),
        KeyCode::Char('k') | KeyCode::Up => focus(Side::Up),
        KeyCode::Char('l') | KeyCode::Right => focus(Side::Right),

        KeyCode::Char('H') => resize(Side::Left),
        KeyCode::Char('J') => resize(Side::Down),
        KeyCode::Char('K') => resize(Side::Up),
        KeyCode::Char('L') => resize(Side::Right),

        _ if same_key(key, PREFIX) => Command::Send(PREFIX),
        // An unknown command does nothing, rather than reaching the pane as
        // a key the user did not mean to press.
        _ => Command::Ignore,
    }
}

/// Move the focus one pane in a direction.
const fn focus(side: Side) -> Command {
    Command::Ask(Request::FocusSide { side })
}

/// Move the focused pane's edge one step.
const fn resize(side: Side) -> Command {
    Command::Ask(Request::ResizePane {
        side,
        cells: RESIZE_CELLS,
    })
}

/// Show the tab `by` positions along, wrapping around.
fn step_tab(view: &SessionView, by: isize) -> Command {
    match step(view.active_tab, view.tabs.len(), by) {
        Some(index) => Command::Ask(Request::SelectTab { index }),
        None => Command::Ignore,
    }
}

/// Show the workspace `by` positions along, wrapping around.
fn step_workspace(view: &SessionView, by: isize) -> Command {
    match step(view.active_workspace, view.workspaces.len(), by) {
        Some(index) => Command::Ask(Request::SelectWorkspace { index }),
        None => Command::Ignore,
    }
}

/// The position `by` steps from `from` in a list of `len`, wrapping around.
fn step(from: usize, len: usize, by: isize) -> Option<usize> {
    if len <= 1 {
        return None;
    }
    let len = isize::try_from(len).ok()?;
    let from = isize::try_from(from).ok()?;
    let next = (from + by).rem_euclid(len);
    usize::try_from(next).ok()
}

/// Decide what a mouse event means.
///
/// A program that asked for mouse reporting is sent the events inside its own
/// pane, because that is what makes a scrollable or clickable program work.
/// Everything else is otag's: a click picks out the pane, the tab or the
/// workspace it landed on.
fn mouse_command(event: MouseEvent, view: &SessionView, screens: &Screens) -> Command {
    let (x, y) = (event.column, event.row);

    let Some(pane) = view.pane_at(x, y) else {
        // Not on a pane, so it is one of otag's own parts — or nothing.
        if !matches!(event.kind, MouseEventKind::Down(_)) {
            return Command::Ignore;
        }
        if let Some(index) = session::tab_at(view, x, y) {
            return Command::Ask(Request::SelectTab { index });
        }
        if let Some(index) = session::workspace_at(view, x, y) {
            return Command::Ask(Request::SelectWorkspace { index });
        }
        return Command::Ignore;
    };

    if view.focus != Some(pane) {
        // A click moves the focus; anything else over an unfocused pane is
        // not worth acting on.
        return if matches!(event.kind, MouseEventKind::Down(_)) {
            Command::Ask(Request::FocusPane { pane })
        } else {
            Command::Ignore
        };
    }

    let Some(area) = view.pane(pane).map(|pane| pane.area.inset(BORDER)) else {
        return Command::Ignore;
    };
    if !area.contains(x, y) {
        // The border belongs to otag, and the pane it belongs to is already
        // focused.
        return Command::Ignore;
    }

    let modes = screens
        .get(&pane)
        .map(ScreenState::modes)
        .unwrap_or_default();
    match keys::encode_mouse(event, modes, x - area.x, y - area.y) {
        Some(bytes) => Command::Ask(Request::Input { bytes }),
        None => Command::Ignore,
    }
}

/// Whether two key presses are the same key with the same modifiers.
fn same_key(left: KeyEvent, right: KeyEvent) -> bool {
    left.code == right.code && left.modifiers == right.modifiers
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
    use otag_core::frame::{CursorState, InputModes, MouseEncoding, MouseMode, ScreenFrame};
    use otag_core::layout::Area;
    use otag_core::pane::PaneId;
    use otag_core::proto::{PaneView, TabView, WorkspaceView};
    use otag_core::workspace::{TabId, WorkspaceId};

    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn pane_id(id: u64) -> PaneId {
        let mut pane = PaneId::FIRST;
        for _ in 1..id {
            pane = pane.next();
        }
        pane
    }

    /// Two panes side by side, two tabs, two workspaces; the right pane
    /// focused.
    fn view() -> SessionView {
        SessionView {
            generation: 1,
            size: PaneSize::new(24, 80),
            sidebar: Area::new(0, 0, 16, 24),
            tabs_area: Area::new(16, 0, 64, 1),
            content: Area::new(16, 1, 64, 23),
            workspaces: vec![
                WorkspaceView {
                    id: WorkspaceId::FIRST,
                    name: "app".to_owned(),
                    tabs: 2,
                    panes: 2,
                },
                WorkspaceView {
                    id: WorkspaceId::FIRST.next(),
                    name: "api".to_owned(),
                    tabs: 1,
                    panes: 1,
                },
            ],
            active_workspace: 0,
            tabs: vec![
                TabView {
                    id: TabId::FIRST,
                    title: "shell".to_owned(),
                    panes: 2,
                },
                TabView {
                    id: TabId::FIRST.next(),
                    title: "build".to_owned(),
                    panes: 1,
                },
            ],
            active_tab: 0,
            panes: vec![
                PaneView {
                    id: pane_id(1),
                    area: Area::new(16, 1, 32, 23),
                    title: "shell".to_owned(),
                    frame: None,
                    exit: None,
                },
                PaneView {
                    id: pane_id(2),
                    area: Area::new(48, 1, 32, 23),
                    title: "shell".to_owned(),
                    frame: None,
                    exit: None,
                },
            ],
            focus: Some(pane_id(2)),
        }
    }

    fn sent(command: Command) -> KeyEvent {
        match command {
            Command::Send(key) => key,
            _ => panic!("expected a key to be sent to the pane"),
        }
    }

    fn asked(command: Command) -> Request {
        match command {
            Command::Ask(request) => request,
            _ => panic!("expected a request to the daemon"),
        }
    }

    fn after_prefix(code: KeyCode) -> Command {
        let mut prefix_held = false;
        assert!(matches!(
            command(PREFIX, &mut prefix_held, &view()),
            Command::Ignore
        ));
        assert!(prefix_held);
        let command = command(key(code), &mut prefix_held, &view());
        assert!(!prefix_held, "the prefix is spent either way");
        command
    }

    #[test]
    fn ordinary_keys_go_straight_to_the_focused_pane() {
        let mut prefix_held = false;
        assert_eq!(
            sent(command(key(KeyCode::Char('a')), &mut prefix_held, &view())),
            key(KeyCode::Char('a'))
        );
        assert!(!prefix_held);
    }

    #[test]
    fn the_prefix_twice_sends_the_prefix_key_itself() {
        // A program in the pane that uses this key can still receive it.
        let mut prefix_held = true;
        assert_eq!(sent(command(PREFIX, &mut prefix_held, &view())), PREFIX);
        assert!(!prefix_held);
    }

    #[test]
    fn the_prefix_followed_by_d_detaches() {
        assert!(matches!(after_prefix(KeyCode::Char('d')), Command::Detach));
    }

    #[test]
    fn the_split_keys_ask_for_a_split_each_way() {
        assert_eq!(
            asked(after_prefix(KeyCode::Char('%'))),
            Request::Split {
                direction: Direction::LeftRight
            }
        );
        assert_eq!(
            asked(after_prefix(KeyCode::Char('"'))),
            Request::Split {
                direction: Direction::TopBottom
            }
        );
    }

    #[test]
    fn the_tab_keys_add_and_choose_tabs() {
        assert_eq!(asked(after_prefix(KeyCode::Char('c'))), Request::NewTab);
        assert_eq!(
            asked(after_prefix(KeyCode::Char('n'))),
            Request::SelectTab { index: 1 },
            "the next tab after the first"
        );
        assert_eq!(
            asked(after_prefix(KeyCode::Char('p'))),
            Request::SelectTab { index: 1 },
            "stepping back from the first wraps to the last"
        );
        assert_eq!(
            asked(after_prefix(KeyCode::Char('2'))),
            Request::SelectTab { index: 1 },
            "tabs are numbered from one on screen"
        );
    }

    #[test]
    fn a_number_with_no_tab_behind_it_does_nothing() {
        assert!(matches!(after_prefix(KeyCode::Char('7')), Command::Ignore));
    }

    #[test]
    fn the_workspace_keys_add_and_choose_workspaces() {
        assert!(matches!(
            after_prefix(KeyCode::Char('w')),
            Command::NewWorkspace
        ));
        assert_eq!(
            asked(after_prefix(KeyCode::Char(')'))),
            Request::SelectWorkspace { index: 1 }
        );
        assert_eq!(
            asked(after_prefix(KeyCode::Char('('))),
            Request::SelectWorkspace { index: 1 },
            "stepping back wraps around"
        );
    }

    #[test]
    fn stepping_through_a_list_of_one_does_nothing() {
        let mut only = view();
        only.tabs.truncate(1);
        let mut prefix_held = true;
        assert!(matches!(
            command(key(KeyCode::Char('n')), &mut prefix_held, &only),
            Command::Ignore
        ));
    }

    #[test]
    fn the_focus_keys_move_between_panes() {
        for (code, side) in [
            (KeyCode::Char('h'), Side::Left),
            (KeyCode::Char('j'), Side::Down),
            (KeyCode::Char('k'), Side::Up),
            (KeyCode::Char('l'), Side::Right),
            (KeyCode::Left, Side::Left),
            (KeyCode::Down, Side::Down),
            (KeyCode::Up, Side::Up),
            (KeyCode::Right, Side::Right),
        ] {
            assert_eq!(asked(after_prefix(code)), Request::FocusSide { side });
        }
        assert_eq!(asked(after_prefix(KeyCode::Char('o'))), Request::FocusNext);
    }

    #[test]
    fn the_resize_keys_move_the_focused_pane_s_edges() {
        for (code, side) in [
            (KeyCode::Char('H'), Side::Left),
            (KeyCode::Char('J'), Side::Down),
            (KeyCode::Char('K'), Side::Up),
            (KeyCode::Char('L'), Side::Right),
        ] {
            assert_eq!(
                asked(after_prefix(code)),
                Request::ResizePane {
                    side,
                    cells: RESIZE_CELLS
                }
            );
        }
    }

    #[test]
    fn a_resize_key_with_shift_reported_as_a_modifier_still_works() {
        // Terminals differ in whether they report shift alongside the capital
        // letter it produced.
        let mut prefix_held = true;
        let shifted = KeyEvent::new(KeyCode::Char('L'), KeyModifiers::SHIFT);
        assert_eq!(
            asked(command(shifted, &mut prefix_held, &view())),
            Request::ResizePane {
                side: Side::Right,
                cells: RESIZE_CELLS
            }
        );
    }

    #[test]
    fn the_sidebar_can_be_hidden() {
        assert_eq!(
            asked(after_prefix(KeyCode::Char('z'))),
            Request::ToggleSidebar
        );
    }

    #[test]
    fn an_unknown_command_after_the_prefix_does_nothing() {
        // Reaching the pane as a key the user did not mean to press would be
        // worse than doing nothing.
        assert!(matches!(after_prefix(KeyCode::Char('q')), Command::Ignore));
    }

    #[test]
    fn a_command_key_with_control_is_not_a_command() {
        let mut prefix_held = true;
        let modified = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL);
        assert!(matches!(
            command(modified, &mut prefix_held, &view()),
            Command::Ignore
        ));
    }

    #[test]
    fn the_detach_key_reaches_the_pane_when_the_prefix_was_not_pressed() {
        let mut prefix_held = false;
        assert_eq!(
            sent(command(key(KeyCode::Char('d')), &mut prefix_held, &view())),
            key(KeyCode::Char('d'))
        );
    }

    fn click(x: u16, y: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn screen_with(modes: InputModes) -> ScreenState {
        let size = PaneSize::new(21, 30);
        let mut screen = ScreenState::new(size);
        screen.apply(&ScreenFrame {
            size,
            cursor: CursorState::default(),
            modes,
            full: true,
            rows: Vec::new(),
        });
        screen
    }

    #[test]
    fn a_click_on_another_pane_focuses_it() {
        assert_eq!(
            asked(mouse_command(click(20, 5), &view(), &Screens::new())),
            Request::FocusPane { pane: pane_id(1) }
        );
    }

    #[test]
    fn a_click_on_the_tab_bar_chooses_the_tab() {
        assert_eq!(
            asked(mouse_command(click(24, 0), &view(), &Screens::new())),
            Request::SelectTab { index: 1 }
        );
    }

    #[test]
    fn a_click_on_the_sidebar_chooses_the_workspace() {
        assert_eq!(
            asked(mouse_command(click(3, 2), &view(), &Screens::new())),
            Request::SelectWorkspace { index: 1 }
        );
    }

    #[test]
    fn a_click_on_nothing_in_particular_does_nothing() {
        assert!(matches!(
            mouse_command(click(3, 20), &view(), &Screens::new()),
            Command::Ignore
        ));
    }

    #[test]
    fn a_click_in_the_focused_pane_goes_to_a_program_that_asked_for_it() {
        let mut screens = Screens::new();
        screens.insert(
            pane_id(2),
            screen_with(InputModes {
                mouse: MouseMode::Press,
                mouse_encoding: MouseEncoding::Sgr,
                ..InputModes::default()
            }),
        );

        // The pane's box starts at column 48 and row 1, so the first cell
        // inside it is at column 49, row 2.
        let request = asked(mouse_command(click(52, 4), &view(), &screens));
        let Request::Input { bytes } = request else {
            panic!("expected the click to reach the program");
        };
        assert_eq!(
            String::from_utf8(bytes).expect("the report is text"),
            "\x1b[<0;4;3M",
            "counted from one inside the pane, not in the terminal"
        );
    }

    #[test]
    fn a_click_in_the_focused_pane_is_dropped_when_no_program_asked_for_it() {
        let mut screens = Screens::new();
        screens.insert(pane_id(2), screen_with(InputModes::default()));
        assert!(matches!(
            mouse_command(click(52, 4), &view(), &screens),
            Command::Ignore
        ));
    }

    #[test]
    fn a_click_on_a_pane_s_border_is_otag_s_own() {
        let mut screens = Screens::new();
        screens.insert(
            pane_id(2),
            screen_with(InputModes {
                mouse: MouseMode::Press,
                ..InputModes::default()
            }),
        );
        // Column 48 is the focused pane's own border.
        assert!(matches!(
            mouse_command(click(48, 5), &view(), &screens),
            Command::Ignore
        ));
    }

    #[test]
    fn moving_the_mouse_over_an_unfocused_pane_changes_nothing() {
        let moved = MouseEvent {
            kind: MouseEventKind::Moved,
            column: 20,
            row: 5,
            modifiers: KeyModifiers::NONE,
        };
        assert!(matches!(
            mouse_command(moved, &view(), &Screens::new()),
            Command::Ignore
        ));
    }

    #[test]
    fn a_view_without_a_frame_for_a_pane_keeps_what_the_client_had() {
        let mut client = Client::new(PaneSize::new(24, 80));
        let mut view = view();
        view.panes[0].frame = Some(ScreenFrame {
            size: PaneSize::new(21, 30),
            cursor: CursorState::default(),
            modes: InputModes::default(),
            full: true,
            rows: vec![otag_core::frame::ScreenRow {
                index: 0,
                runs: vec![otag_core::frame::StyledRun {
                    text: "kept".to_owned(),
                    style: otag_core::frame::CellStyle::default(),
                }],
            }],
        });
        client.apply_view(view.clone());
        assert_eq!(client.screens[&pane_id(1)].row_text(0), "kept");

        // A second view says nothing about the pane's screen.
        let mut without = view;
        without.panes[0].frame = None;
        client.apply_view(without);
        assert_eq!(client.screens[&pane_id(1)].row_text(0), "kept");
    }

    #[test]
    fn a_pane_that_leaves_the_view_leaves_the_client_too() {
        let mut client = Client::new(PaneSize::new(24, 80));
        client.apply_view(view());
        assert_eq!(client.screens.len(), 2);

        let mut one = view();
        one.panes.truncate(1);
        client.apply_view(one);
        assert_eq!(client.screens.len(), 1);
        assert!(!client.screens.contains_key(&pane_id(2)));
    }

    #[test]
    fn a_frame_for_a_pane_the_client_has_not_seen_is_still_kept() {
        let mut client = Client::new(PaneSize::new(24, 80));
        client.apply_frame(
            pane_id(5),
            &ScreenFrame {
                size: PaneSize::new(2, 4),
                cursor: CursorState::default(),
                modes: InputModes::default(),
                full: true,
                rows: Vec::new(),
            },
        );
        assert!(client.screens.contains_key(&pane_id(5)));
    }

    #[test]
    fn keys_are_encoded_for_the_pane_they_are_going_to() {
        let mut client = Client::new(PaneSize::new(24, 80));
        client.apply_view(view());
        client.screens.insert(
            pane_id(2),
            screen_with(InputModes {
                application_cursor: true,
                ..InputModes::default()
            }),
        );
        assert!(focused_modes(&client).application_cursor);

        // With no focused pane there are no modes to apply, rather than a
        // panic or the wrong pane's.
        client.view.focus = None;
        assert_eq!(focused_modes(&client), InputModes::default());
    }
}
