//! The panes a daemon is running.
//!
//! Each pane owns a task that reads its pseudo-terminal, feeds the emulator
//! and publishes the resulting screen. Clients never touch a pane directly:
//! they send it commands and watch what it publishes, so a client going away
//! — or arriving late, or falling behind — cannot disturb the program running
//! on it.
//!
//! Screen updates are published through a watch channel, which holds only the
//! newest screen. A client that cannot keep up therefore skips intermediate
//! screens instead of queueing them, and still receives a correct picture:
//! every client diffs against the last screen it was actually sent.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{mpsc, watch};

use crate::frame::ScreenState;
use crate::pane::{PaneExit, PaneId, PaneSize, PaneSpec};
use crate::pty::{Pty, PtyEvent};
use crate::screen::Screen;
use crate::{Error, Result};

/// Shortest time between two published screens.
///
/// A program can write far more often than a person can read. Updates within
/// one interval are merged, which keeps a burst of output from turning into a
/// burst of frames on the socket.
const FRAME_INTERVAL: Duration = Duration::from_millis(16);

/// How many commands may wait for a pane before a sender has to wait.
const COMMAND_QUEUE: usize = 64;

/// What a client can ask a pane to do.
#[derive(Debug)]
enum PaneCommand {
    /// Write these bytes to the program's input.
    Input(Vec<u8>),
    /// The pane is now this size.
    Resize(PaneSize),
}

/// What a pane publishes to everyone watching it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneView {
    /// The pane's screen.
    pub screen: ScreenState,
    /// How the program ended, once it has.
    pub exit: Option<PaneExit>,
}

/// A handle on one running pane.
#[derive(Debug, Clone)]
pub struct PaneHandle {
    id: PaneId,
    commands: mpsc::Sender<PaneCommand>,
    view: watch::Receiver<Arc<PaneView>>,
}

impl PaneHandle {
    /// Which pane this is.
    #[must_use]
    pub const fn id(&self) -> PaneId {
        self.id
    }

    /// The pane's current screen and exit state.
    #[must_use]
    pub fn view(&self) -> Arc<PaneView> {
        Arc::clone(&self.view.borrow())
    }

    /// Whether the pane's program has ended.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.view.borrow().exit.is_some()
    }

    /// A receiver that wakes whenever the pane publishes a new screen.
    #[must_use]
    pub fn watch(&self) -> watch::Receiver<Arc<PaneView>> {
        self.view.clone()
    }

    /// Write bytes to the program's input.
    pub async fn input(&self, bytes: Vec<u8>) -> Result<()> {
        self.send(PaneCommand::Input(bytes)).await
    }

    /// Tell the pane it has a new size.
    pub async fn resize(&self, size: PaneSize) -> Result<()> {
        self.send(PaneCommand::Resize(size)).await
    }

    async fn send(&self, command: PaneCommand) -> Result<()> {
        self.commands
            .send(command)
            .await
            .map_err(|_| Error::PaneGone { id: Some(self.id) })
    }
}

/// What `otag ls` reports about a pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneSummary {
    /// Which pane this is.
    pub id: PaneId,
    /// Its current size.
    pub size: PaneSize,
    /// How its program ended, if it has.
    pub exit: Option<PaneExit>,
}

/// Every pane one daemon is running.
///
/// This milestone runs a single pane; the type is already a collection so
/// that splits and workspaces do not change how clients reach a pane.
#[derive(Debug, Default)]
pub struct Session {
    state: Mutex<SessionState>,
}

#[derive(Debug, Default)]
struct SessionState {
    panes: BTreeMap<PaneId, PaneHandle>,
    next_id: Option<PaneId>,
}

impl Session {
    /// A daemon with no panes.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The pane a client should attach to, starting one if there is none.
    ///
    /// A pane whose program has ended is replaced, so that attaching after a
    /// shell exited gives a working pane rather than a dead one.
    pub fn attach_pane(&self, spec: &PaneSpec, size: PaneSize) -> Result<PaneHandle> {
        let Ok(mut state) = self.state.lock() else {
            return Err(Error::PaneGone { id: None });
        };

        if let Some(handle) = state
            .panes
            .values()
            .find(|handle| !handle.is_finished())
            .cloned()
        {
            return Ok(handle);
        }
        state.panes.retain(|_, handle| !handle.is_finished());

        let id = state.next_id.unwrap_or(PaneId::FIRST);
        let handle = spawn_pane(id, spec, size)?;
        state.next_id = Some(id.next());
        state.panes.insert(id, handle.clone());
        Ok(handle)
    }

    /// The pane with this identifier, if the daemon has it.
    #[must_use]
    pub fn pane(&self, id: PaneId) -> Option<PaneHandle> {
        self.state.lock().ok()?.panes.get(&id).cloned()
    }

    /// Every pane, in creation order.
    #[must_use]
    pub fn summaries(&self) -> Vec<PaneSummary> {
        let Ok(state) = self.state.lock() else {
            return Vec::new();
        };
        state
            .panes
            .values()
            .map(|handle| {
                let view = handle.view();
                PaneSummary {
                    id: handle.id(),
                    size: view.screen.size(),
                    exit: view.exit.clone(),
                }
            })
            .collect()
    }

    /// Whether the daemon has no panes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.state.lock().is_ok_and(|state| state.panes.is_empty())
    }
}

/// Start a pane and the task driving it.
fn spawn_pane(id: PaneId, spec: &PaneSpec, size: PaneSize) -> Result<PaneHandle> {
    let (pty, events) = Pty::spawn(id, spec, size)?;
    let screen = Screen::new(size);

    let (view_tx, view_rx) = watch::channel(Arc::new(PaneView {
        screen: screen.state(),
        exit: None,
    }));
    let (commands_tx, commands_rx) = mpsc::channel(COMMAND_QUEUE);

    tokio::spawn(run_pane(id, pty, screen, events, commands_rx, view_tx));

    Ok(PaneHandle {
        id,
        commands: commands_tx,
        view: view_rx,
    })
}

/// Drive one pane until its program ends or every client is gone.
async fn run_pane(
    id: PaneId,
    mut pty: Pty,
    mut screen: Screen,
    mut events: mpsc::Receiver<PtyEvent>,
    mut commands: mpsc::Receiver<PaneCommand>,
    view: watch::Sender<Arc<PaneView>>,
) {
    let mut ticker = tokio::time::interval(FRAME_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut dirty = false;

    loop {
        tokio::select! {
            event = events.recv() => match event {
                Some(PtyEvent::Output(bytes)) => {
                    screen.process(&bytes);
                    dirty = true;

                    // Programs ask the terminal questions; the answers have
                    // to go back as input or the program waits for them.
                    let replies = screen.take_replies();
                    if !replies.is_empty() && let Err(error) = pty.write(replies).await {
                        tracing::debug!(%error, pane = %id, "cannot answer a pane's query");
                    }
                }
                Some(PtyEvent::Exited(exit)) => {
                    tracing::info!(pane = %id, %exit, "pane finished");
                    publish(&view, &screen, Some(exit));
                    return;
                }
                // The reading thread is gone without reporting an exit, which
                // means the pane cannot produce anything more.
                None => {
                    publish(&view, &screen, Some(PaneExit { code: None, signal: None }));
                    return;
                }
            },

            command = commands.recv() => match command {
                Some(PaneCommand::Input(bytes)) => {
                    if let Err(error) = pty.write(bytes).await {
                        tracing::debug!(%error, pane = %id, "cannot write to a pane");
                    }
                }
                Some(PaneCommand::Resize(size)) => {
                    if let Err(error) = pty.resize(size) {
                        tracing::warn!(%error, pane = %id, "cannot resize a pane");
                    } else {
                        screen.resize(size);
                        dirty = true;
                    }
                }
                // Nobody can reach this pane any more. The program is killed
                // when the pseudo-terminal is dropped.
                None => return,
            },

            _ = ticker.tick(), if dirty => {
                dirty = false;
                publish(&view, &screen, None);
            }
        }
    }
}

/// Publish the current screen, but only when it actually changed.
fn publish(view: &watch::Sender<Arc<PaneView>>, screen: &Screen, exit: Option<PaneExit>) {
    let next = PaneView {
        screen: screen.state(),
        exit,
    };
    if **view.borrow() == next {
        return;
    }
    // A failure means every watcher is gone, which is not a problem: the
    // newest screen stays available for whoever attaches next.
    let _ = view.send(Arc::new(next));
}
