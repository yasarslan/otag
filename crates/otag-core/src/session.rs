//! Everything one daemon is running: workspaces, tabs, panes.
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
//!
//! Above the panes sits the structure a client draws: workspaces, their tabs,
//! and each tab's layout. A change to any of it bumps a generation counter
//! that clients watch, which is how they learn to ask for a new view instead
//! of being sent one they then have to reconcile.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{mpsc, watch};

use crate::frame::ScreenState;
use crate::layout::{self, Area, BORDER, Chrome, Direction, Side};
use crate::pane::{PaneExit, PaneId, PaneSize, PaneSpec};
use crate::proto::{PaneView, SessionView, TabView, WorkspaceView};
use crate::pty::{Pty, PtyEvent};
use crate::screen::Screen;
use crate::snapshot::{self, PaneSnapshot, Snapshot, TabSnapshot, WorkspaceSnapshot};
use crate::workspace::{Tab, TabId, Workspace, WorkspaceId};
use crate::{Error, Result};

/// Shortest time between two published screens.
///
/// A program can write far more often than a person can read. Updates within
/// one interval are merged, which keeps a burst of output from turning into a
/// burst of frames on the socket.
const FRAME_INTERVAL: Duration = Duration::from_millis(16);

/// How many commands may wait for a pane before a sender has to wait.
const COMMAND_QUEUE: usize = 64;

/// What a tab is called before anything better is known about it.
const SHELL_TITLE: &str = "shell";

/// The line otag draws between what a restored pane used to show and what
/// its program prints from now on.
const RESTORED_DIVIDER: &str = "\x1b[2m\u{2500}\u{2500} restored \u{2500}\u{2500}\x1b[0m\r\n";

/// What a client can ask a pane to do.
#[derive(Debug)]
enum PaneCommand {
    /// Write these bytes to the program's input.
    Input(Vec<u8>),
    /// The pane is now this size.
    Resize(PaneSize),
    /// Report the lines the pane has shown, for a snapshot.
    History(usize, tokio::sync::oneshot::Sender<Vec<String>>),
}

/// What a pane publishes to everyone watching it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneState {
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
    view: watch::Receiver<Arc<PaneState>>,
}

impl PaneHandle {
    /// Which pane this is.
    #[must_use]
    pub const fn id(&self) -> PaneId {
        self.id
    }

    /// The pane's current screen and exit state.
    #[must_use]
    pub fn snapshot(&self) -> Arc<PaneState> {
        Arc::clone(&self.view.borrow())
    }

    /// Whether the pane's program has ended.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.view.borrow().exit.is_some()
    }

    /// A receiver that wakes whenever the pane publishes a new screen.
    #[must_use]
    pub fn watch(&self) -> watch::Receiver<Arc<PaneState>> {
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

    /// The lines the pane has shown, at most `max` of them, oldest first.
    ///
    /// Asked of the pane's own task, because that is what owns the emulator.
    pub async fn history(&self, max: usize) -> Result<Vec<String>> {
        let (lines_tx, lines_rx) = tokio::sync::oneshot::channel();
        self.send(PaneCommand::History(max, lines_tx)).await?;
        lines_rx
            .await
            .map_err(|_| Error::PaneGone { id: Some(self.id) })
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
    /// Name of the workspace it belongs to.
    pub workspace: String,
    /// What it is running.
    pub title: String,
    /// How its program ended, if it has.
    pub exit: Option<PaneExit>,
}

/// Everything needed to start one pane.
struct Starting<'a> {
    id: PaneId,
    workspace: WorkspaceId,
    cwd: Option<&'a Path>,
    /// What to run, empty for the user's shell.
    command: &'a [String],
    title: &'a str,
    /// What the pane showed before, when it is being restored.
    restored: &'a [String],
}

/// One running pane, with what the interface says about it.
#[derive(Debug)]
struct Pane {
    handle: PaneHandle,
    title: String,
    workspace: WorkspaceId,
    /// Where it was started, so a snapshot can start it there again.
    cwd: Option<PathBuf>,
    /// What it was started with, empty for the user's shell.
    command: Vec<String>,
}

/// Everything one daemon owns.
#[derive(Debug)]
pub struct Session {
    /// What every new pane runs, and the environment it runs in.
    template: PaneSpec,
    /// Bumped on every structural change, so clients know to ask for a view.
    changes: watch::Sender<u64>,
    state: Mutex<State>,
}

#[derive(Debug)]
struct State {
    workspaces: Vec<Workspace>,
    active: usize,
    panes: BTreeMap<PaneId, Pane>,
    next_pane: PaneId,
    next_workspace: WorkspaceId,
    next_tab: TabId,
    /// Size of the terminal the panes are laid out for.
    size: PaneSize,
    /// Whether the sidebar takes columns from the panes.
    sidebar: bool,
    generation: u64,
    /// How the pane that ended most recently ended, for the client's last
    /// word on a session that is over.
    last_exit: Option<PaneExit>,
}

impl State {
    /// Where otag's furniture goes and what is left for panes.
    fn chrome(&self) -> Chrome {
        Chrome::of(self.size, self.sidebar)
    }

    /// The workspace that is shown.
    fn workspace(&self) -> Option<&Workspace> {
        self.workspaces.get(self.active)
    }

    /// The workspace that is shown, to change.
    fn workspace_mut(&mut self) -> Option<&mut Workspace> {
        self.workspaces.get_mut(self.active)
    }

    /// The tab that is shown.
    fn tab(&self) -> Option<&Tab> {
        self.workspace().and_then(Workspace::tab)
    }

    /// The tab that is shown, to change.
    fn tab_mut(&mut self) -> Option<&mut Tab> {
        self.workspace_mut().and_then(Workspace::tab_mut)
    }

    /// Where the visible panes are, border included.
    fn areas(&self) -> Vec<(PaneId, Area)> {
        let content = self.chrome().content;
        self.tab()
            .map(|tab| tab.layout().areas(content))
            .unwrap_or_default()
    }

    /// The pane the user is typing into.
    fn focus(&self) -> Option<PaneId> {
        self.tab().map(Tab::focus)
    }

    /// Panes whose program is being told the wrong size, and the size they
    /// should be told instead.
    ///
    /// Only the visible panes are resized: a pane in another tab keeps the
    /// size it was drawn at until it is shown again, which is also what its
    /// program is expecting.
    fn pending_resizes(&self) -> Vec<(PaneHandle, PaneSize)> {
        self.areas()
            .into_iter()
            .filter_map(|(id, area)| {
                let pane = self.panes.get(&id)?;
                let size = area.inset(BORDER).size();
                (pane.handle.snapshot().screen.size() != size).then(|| (pane.handle.clone(), size))
            })
            .collect()
    }

    /// Names of the workspaces, for giving a new one a name of its own.
    fn names(&self) -> Vec<String> {
        self.workspaces
            .iter()
            .map(|workspace| workspace.name().to_owned())
            .collect()
    }
}

impl Session {
    /// A daemon with no panes, whose panes will run `template`.
    #[must_use]
    pub fn new(template: PaneSpec) -> Arc<Self> {
        Arc::new(Self {
            template,
            changes: watch::Sender::new(0),
            state: Mutex::new(State {
                workspaces: Vec::new(),
                active: 0,
                panes: BTreeMap::new(),
                next_pane: PaneId::FIRST,
                next_workspace: WorkspaceId::FIRST,
                next_tab: TabId::FIRST,
                size: PaneSize::DEFAULT,
                sidebar: true,
                generation: 0,
                last_exit: None,
            }),
        })
    }

    /// A receiver that wakes on every structural change.
    #[must_use]
    pub fn changes(&self) -> watch::Receiver<u64> {
        self.changes.subscribe()
    }

    /// Attach a client: lay the session out for its terminal, starting a
    /// workspace with one pane if the daemon has none.
    pub async fn attach(self: &Arc<Self>, size: PaneSize, cwd: Option<&Path>) -> Result<()> {
        {
            let mut state = self.lock()?;
            state.size = size;
            if state.workspaces.is_empty() {
                self.start_workspace(&mut state, cwd)?;
            }
        }
        // Always committed, even when nothing was started: the client's
        // terminal decides the sizes, and it may differ from the last one's.
        self.commit(true).await;
        Ok(())
    }

    /// Lay the session out for a terminal of a new size.
    pub async fn resize(self: &Arc<Self>, size: PaneSize) {
        let changed = match self.lock() {
            Ok(mut state) => {
                let changed = state.size != size;
                state.size = size;
                changed
            }
            Err(_) => false,
        };
        self.commit(changed).await;
    }

    /// Show or hide the sidebar, giving its columns to the panes or taking
    /// them back.
    pub async fn toggle_sidebar(self: &Arc<Self>) {
        if let Ok(mut state) = self.lock() {
            state.sidebar = !state.sidebar;
        }
        self.commit(true).await;
    }

    /// Divide the focused pane and start a pane in the new half.
    pub async fn split(self: &Arc<Self>, direction: Direction) -> Result<()> {
        {
            let mut state = self.lock()?;
            let Some(workspace) = state.workspace().map(|workspace| workspace.id()) else {
                return Err(Error::PaneGone { id: None });
            };
            let cwd = state
                .workspace()
                .and_then(|workspace| workspace.cwd().map(Path::to_path_buf));

            let pane = self.start_pane(&mut state, workspace, cwd.as_deref())?;
            let placed = state
                .tab_mut()
                .is_some_and(|tab| tab.split(direction, pane));
            if !placed {
                // The focused pane went missing between the split being
                // asked for and being carried out, so the new pane has
                // nowhere to go. Dropping its handle ends its program.
                state.panes.remove(&pane);
                return Err(Error::NoPanes);
            }
        }
        self.commit(true).await;
        Ok(())
    }

    /// Add a tab with one pane to the active workspace, and show it.
    pub async fn new_tab(self: &Arc<Self>) -> Result<()> {
        {
            let mut state = self.lock()?;
            let Some(workspace) = state.workspace().map(|workspace| workspace.id()) else {
                return Err(Error::PaneGone { id: None });
            };
            let cwd = state
                .workspace()
                .and_then(|workspace| workspace.cwd().map(Path::to_path_buf));

            let pane = self.start_pane(&mut state, workspace, cwd.as_deref())?;
            let id = state.next_tab;
            state.next_tab = id.next();
            let tab = Tab::new(id, SHELL_TITLE, pane);
            if let Some(workspace) = state.workspace_mut() {
                workspace.add_tab(tab);
            }
        }
        self.commit(true).await;
        Ok(())
    }

    /// Add a workspace with one tab and one pane, and show it.
    pub async fn new_workspace(self: &Arc<Self>, cwd: Option<&Path>) -> Result<()> {
        {
            let mut state = self.lock()?;
            self.start_workspace(&mut state, cwd)?;
        }
        self.commit(true).await;
        Ok(())
    }

    /// Show the tab at `index` of the active workspace.
    pub async fn select_tab(self: &Arc<Self>, index: usize) {
        let changed = self
            .lock()
            .ok()
            .and_then(|mut state| {
                state
                    .workspace_mut()
                    .map(|workspace| workspace.select(index))
            })
            .unwrap_or(false);
        self.commit(changed).await;
    }

    /// Show the workspace at `index`.
    pub async fn select_workspace(self: &Arc<Self>, index: usize) {
        let changed = match self.lock() {
            Ok(mut state) if index < state.workspaces.len() && index != state.active => {
                state.active = index;
                true
            }
            _ => false,
        };
        self.commit(changed).await;
    }

    /// Type into `pane` from now on, if the active tab holds it.
    pub async fn focus_pane(self: &Arc<Self>, pane: PaneId) {
        let changed = self
            .lock()
            .ok()
            .and_then(|mut state| state.tab_mut().map(|tab| tab.focus_pane(pane)))
            .unwrap_or(false);
        // Nothing moves, but every client has to redraw the borders.
        self.commit(changed).await;
    }

    /// Focus the pane on `side` of the focused one.
    pub async fn focus_side(self: &Arc<Self>, side: Side) {
        let changed = match self.lock() {
            Ok(mut state) => {
                let content = state.chrome().content;
                state
                    .tab_mut()
                    .is_some_and(|tab| tab.focus_side(side, content))
            }
            Err(_) => false,
        };
        self.commit(changed).await;
    }

    /// Focus the next pane of the active tab, wrapping around.
    pub async fn focus_next(self: &Arc<Self>) {
        if let Ok(mut state) = self.lock()
            && let Some(tab) = state.tab_mut()
        {
            tab.focus_next();
        }
        self.commit(true).await;
    }

    /// Move the focused pane's edge on `side` outwards by `cells` cells.
    pub async fn resize_pane(self: &Arc<Self>, side: Side, cells: u16) {
        let changed = match self.lock() {
            Ok(mut state) => {
                let content = state.chrome().content;
                let along = match side.axis() {
                    Direction::TopBottom => content.rows,
                    Direction::LeftRight | Direction::Unknown => content.cols,
                };
                let amount = layout::weight_per_cell(along).saturating_mul(u32::from(cells.max(1)));
                state.tab_mut().is_some_and(|tab| tab.resize(side, amount))
            }
            Err(_) => false,
        };
        self.commit(changed).await;
    }

    /// Write bytes to the focused pane's program.
    pub async fn input(&self, bytes: Vec<u8>) -> Result<()> {
        let pane = self.focused_pane().ok_or(Error::NoPanes)?;
        pane.input(bytes).await
    }

    /// Write bytes to one pane's program.
    pub async fn input_to(&self, id: PaneId, bytes: Vec<u8>) -> Result<()> {
        self.pane(id)
            .ok_or(Error::PaneGone { id: Some(id) })?
            .input(bytes)
            .await
    }

    /// The pane with this identifier, if the daemon has it.
    #[must_use]
    pub fn pane(&self, id: PaneId) -> Option<PaneHandle> {
        let state = self.state.lock().ok()?;
        state.panes.get(&id).map(|pane| pane.handle.clone())
    }

    /// The pane the user is typing into.
    #[must_use]
    pub fn focused_pane(&self) -> Option<PaneHandle> {
        let state = self.state.lock().ok()?;
        let focus = state.focus()?;
        state.panes.get(&focus).map(|pane| pane.handle.clone())
    }

    /// The panes of the active tab, in the order they are drawn.
    #[must_use]
    pub fn visible_panes(&self) -> Vec<PaneId> {
        let Ok(state) = self.state.lock() else {
            return Vec::new();
        };
        state.areas().into_iter().map(|(id, _)| id).collect()
    }

    /// Everything a client has to draw.
    #[must_use]
    pub fn view(&self) -> SessionView {
        let Ok(state) = self.state.lock() else {
            // A poisoned lock means a holder panicked. An empty view says
            // there is nothing to draw, which is better than a panic in
            // every client that asks.
            return SessionView::default();
        };

        let chrome = state.chrome();
        let panes = state
            .areas()
            .into_iter()
            .filter_map(|(id, area)| {
                let pane = state.panes.get(&id)?;
                let snapshot = pane.handle.snapshot();
                Some(PaneView {
                    id,
                    area,
                    title: pane.title.clone(),
                    frame: Some(snapshot.screen.to_frame()),
                    exit: snapshot.exit.clone(),
                })
            })
            .collect();

        SessionView {
            generation: state.generation,
            size: state.size,
            sidebar: chrome.sidebar,
            tabs_area: chrome.tabs,
            content: chrome.content,
            workspaces: state
                .workspaces
                .iter()
                .map(|workspace| WorkspaceView {
                    id: workspace.id(),
                    name: workspace.name().to_owned(),
                    tabs: workspace.tabs().len(),
                    panes: workspace.panes().len(),
                })
                .collect(),
            active_workspace: state.active,
            tabs: state
                .workspace()
                .map(|workspace| {
                    workspace
                        .tabs()
                        .iter()
                        .map(|tab| TabView {
                            id: tab.id(),
                            title: tab.title().to_owned(),
                            panes: tab.panes().len(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            active_tab: state.workspace().map_or(0, Workspace::active),
            panes,
            focus: state.focus(),
        }
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
            .map(|pane| {
                let snapshot = pane.handle.snapshot();
                PaneSummary {
                    id: pane.handle.id(),
                    size: snapshot.screen.size(),
                    workspace: state
                        .workspaces
                        .iter()
                        .find(|workspace| workspace.id() == pane.workspace)
                        .map_or_else(String::new, |workspace| workspace.name().to_owned()),
                    title: pane.title.clone(),
                    exit: snapshot.exit.clone(),
                }
            })
            .collect()
    }

    /// Whether the daemon has no panes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.state.lock().is_ok_and(|state| state.panes.is_empty())
    }

    /// How the pane that ended most recently ended.
    #[must_use]
    pub fn last_exit(&self) -> Option<PaneExit> {
        self.state.lock().ok()?.last_exit.clone()
    }

    /// Everything about the session that is worth keeping, for a snapshot.
    ///
    /// What the panes have shown is not part of it: that has to be asked of
    /// each pane's own task, which [`Session::histories`] does, and is larger
    /// than the rest, so it is saved on its own schedule.
    #[must_use]
    pub fn snapshot(&self) -> Snapshot {
        let Ok(state) = self.state.lock() else {
            return Snapshot::default();
        };

        Snapshot {
            version: snapshot::VERSION,
            size: state.size,
            sidebar: state.sidebar,
            active_workspace: state.active,
            workspaces: state
                .workspaces
                .iter()
                .map(|workspace| WorkspaceSnapshot {
                    id: workspace.id(),
                    name: workspace.name().to_owned(),
                    cwd: workspace.cwd().map(Path::to_path_buf),
                    active_tab: workspace.active(),
                    tabs: workspace
                        .tabs()
                        .iter()
                        .map(|tab| TabSnapshot {
                            id: tab.id(),
                            title: tab.title().to_owned(),
                            focus: tab.focus(),
                            layout: tab.layout().clone(),
                        })
                        .collect(),
                })
                .collect(),
            panes: state
                .panes
                .values()
                .map(|pane| PaneSnapshot {
                    id: pane.handle.id(),
                    title: pane.title.clone(),
                    cwd: pane.cwd.clone(),
                    command: pane.command.clone(),
                })
                .collect(),
        }
    }

    /// The lines each pane has shown, for a snapshot.
    ///
    /// A pane that cannot answer is left out rather than holding up the
    /// others: a snapshot missing one pane's lines still restores the
    /// session.
    pub async fn histories(&self, max: usize) -> Vec<(PaneId, Vec<String>)> {
        let panes = match self.state.lock() {
            Ok(state) => state
                .panes
                .values()
                .map(|pane| pane.handle.clone())
                .collect::<Vec<_>>(),
            Err(_) => return Vec::new(),
        };

        let mut histories = Vec::with_capacity(panes.len());
        for handle in panes {
            match handle.history(max).await {
                Ok(lines) => histories.push((handle.id(), lines)),
                Err(error) => {
                    tracing::debug!(%error, pane = %handle.id(), "cannot read what a pane has shown");
                }
            }
        }
        histories
    }

    /// Put a session back together from a snapshot.
    ///
    /// Every pane is started again, with the lines `history` has for it
    /// shown above a divider. A pane that cannot be started is dropped
    /// from its layout, a tab left with no panes is dropped, and a workspace
    /// left with no tabs is dropped: a snapshot that is partly unusable
    /// restores the part that is, rather than nothing.
    ///
    /// Does nothing when the session already has panes, or when the snapshot
    /// is empty or was written by a newer otag.
    pub async fn restore(
        self: &Arc<Self>,
        snapshot: &Snapshot,
        history: &dyn Fn(PaneId) -> Vec<String>,
    ) -> Result<()> {
        {
            let mut state = self.lock()?;
            if !state.workspaces.is_empty() {
                return Ok(());
            }
            if !snapshot.is_readable() {
                tracing::warn!(
                    version = snapshot.version,
                    "not restoring a session saved by a newer otag"
                );
                return Ok(());
            }
            if snapshot.is_empty() {
                return Ok(());
            }

            state.size = snapshot.size;
            state.sidebar = snapshot.sidebar;
            restore_into(self, &mut state, snapshot, history);
            if state.workspaces.is_empty() {
                tracing::warn!("nothing in the saved session could be restored");
                return Ok(());
            }
            state.active = snapshot
                .active_workspace
                .min(state.workspaces.len().saturating_sub(1));
        }

        self.commit(true).await;
        Ok(())
    }

    /// Take a pane out of the session, dropping a tab or a workspace that is
    /// left with nothing in it.
    async fn close_pane(self: &Arc<Self>, id: PaneId, exit: Option<PaneExit>) {
        {
            let Ok(mut state) = self.lock() else { return };
            if state.panes.remove(&id).is_none() {
                return;
            }
            if exit.is_some() {
                state.last_exit = exit;
            }
            state
                .workspaces
                .retain_mut(|workspace| workspace.close_pane(id));
            state.active = state.active.min(state.workspaces.len().saturating_sub(1));
        }
        self.commit(true).await;
    }

    /// Start a workspace with one tab and one pane, and show it.
    fn start_workspace(self: &Arc<Self>, state: &mut State, cwd: Option<&Path>) -> Result<()> {
        let id = state.next_workspace;
        state.next_workspace = id.next();
        let name = crate::workspace::unique_name(&crate::workspace::name_for(cwd), &state.names());

        let pane = self.start_pane(state, id, cwd)?;
        let tab_id = state.next_tab;
        state.next_tab = tab_id.next();

        let workspace = Workspace::new(
            id,
            name,
            cwd.map(Path::to_path_buf),
            Tab::new(tab_id, SHELL_TITLE, pane),
        );
        state.workspaces.push(workspace);
        state.active = state.workspaces.len().saturating_sub(1);
        Ok(())
    }

    /// Start a pane, and the task that notices when its program ends.
    fn start_pane(
        self: &Arc<Self>,
        state: &mut State,
        workspace: WorkspaceId,
        cwd: Option<&Path>,
    ) -> Result<PaneId> {
        let id = state.next_pane;
        let pane = self.start_pane_as(
            state,
            &Starting {
                id,
                workspace,
                cwd,
                command: &[],
                title: SHELL_TITLE,
                restored: &[],
            },
        )?;
        state.next_pane = id.next();
        state.panes.insert(id, pane);
        Ok(id)
    }

    /// Start one pane with everything about it decided by the caller, which
    /// is what restoring a snapshot needs.
    ///
    /// The pane is not put into the session: the caller does that once it
    /// knows the pane has somewhere to go.
    fn start_pane_as(self: &Arc<Self>, state: &State, starting: &Starting<'_>) -> Result<Pane> {
        let spec = if starting.command.is_empty() {
            self.template.clone().with_cwd_opt(starting.cwd)
        } else {
            // A pane restored with a command of its own still needs the
            // environment otag puts its panes in, so that the agent in it
            // can report back.
            let mut spec = PaneSpec::command(starting.command);
            for (key, value) in self.template.env() {
                spec = spec.with_env(key, value);
            }
            spec.with_cwd_opt(starting.cwd)
        };

        // A pane is started at the size of the whole content area and told
        // its real size as soon as the layout is known, which is one resize
        // before the program has drawn anything.
        let size = state.chrome().content.inset(BORDER).size();
        let handle = spawn_pane(starting.id, &spec, size, starting.restored)?;
        self.reap(handle.clone());

        Ok(Pane {
            handle,
            title: starting.title.to_owned(),
            workspace: starting.workspace,
            cwd: starting.cwd.map(Path::to_path_buf),
            command: starting.command.to_vec(),
        })
    }

    /// The state, as an error rather than a panic when a holder panicked.
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, State>> {
        self.state.lock().map_err(|_| Error::PaneGone { id: None })
    }

    /// Tell the panes their new sizes, then tell the clients to redraw.
    ///
    /// Sizes go out first so that a client drawing the new view is drawing
    /// panes that have already been told how large they are.
    async fn commit(self: &Arc<Self>, changed: bool) {
        if !changed {
            return;
        }

        let resizes = self
            .lock()
            .map(|state| state.pending_resizes())
            .unwrap_or_default();
        for (handle, size) in resizes {
            if let Err(error) = handle.resize(size).await {
                tracing::debug!(%error, pane = %handle.id(), "cannot resize a pane");
            }
        }

        let generation = match self.lock() {
            Ok(mut state) => {
                state.generation = state.generation.saturating_add(1);
                state.generation
            }
            Err(_) => return,
        };
        // Watching clients learn there is a new view to ask for. A send that
        // reaches nobody is not a problem: the view stays available.
        self.changes.send_replace(generation);
    }

    /// Watch a pane and take it out of the session when its program ends.
    fn reap(self: &Arc<Self>, handle: PaneHandle) {
        let session = Arc::clone(self);
        let mut updates = handle.watch();
        tokio::spawn(async move {
            let exit = loop {
                // The borrow is dropped before the await: holding it across
                // one would block the pane's own task.
                let exit = updates.borrow_and_update().exit.clone();
                if let Some(exit) = exit {
                    break Some(exit);
                }
                if updates.changed().await.is_err() {
                    // The pane's task is gone without having reported an
                    // exit, which happens while the daemon shuts down.
                    break None;
                }
            };
            session.close_pane(handle.id(), exit).await;
        });
    }
}

/// Start a pane and the task driving it.
///
/// `restored` is what the pane showed before the daemon was last stopped: it
/// is put on the screen, above a divider, before the program's own output,
/// so that a restored pane looks like the one it replaces rather than like a
/// new one.
fn spawn_pane(
    id: PaneId,
    spec: &PaneSpec,
    size: PaneSize,
    restored: &[String],
) -> Result<PaneHandle> {
    let (pty, events) = Pty::spawn(id, spec, size)?;
    let mut screen = Screen::new(size);
    if !restored.is_empty() {
        screen.process(&restored_bytes(restored));
    }

    let (view_tx, view_rx) = watch::channel(Arc::new(PaneState {
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
    view: watch::Sender<Arc<PaneState>>,
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
                Some(PaneCommand::History(max, answer)) => {
                    // The caller may have given up waiting, which is not a
                    // problem: a snapshot without this pane's lines is
                    // better than a pane that stopped reading its program.
                    let _ = answer.send(screen.history(max));
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

/// Start the panes a snapshot describes and build the workspaces holding
/// them.
///
/// Anything that cannot be started is left out: the point is to restore as
/// much of the session as still works, not to insist on all of it.
fn restore_into(
    session: &Arc<Session>,
    state: &mut State,
    snapshot: &Snapshot,
    history: &dyn Fn(PaneId) -> Vec<String>,
) {
    // Identifiers are never reused, so counting carries on above the highest
    // one the snapshot used.
    state.next_pane = highest(snapshot.panes.iter().map(|pane| pane.id), PaneId::FIRST).next();
    state.next_workspace = highest(
        snapshot.workspaces.iter().map(|workspace| workspace.id),
        WorkspaceId::FIRST,
    )
    .next();
    state.next_tab = highest(
        snapshot
            .workspaces
            .iter()
            .flat_map(|workspace| workspace.tabs.iter().map(|tab| tab.id)),
        TabId::FIRST,
    )
    .next();

    for workspace in &snapshot.workspaces {
        let mut tabs = Vec::with_capacity(workspace.tabs.len());

        for tab in &workspace.tabs {
            let mut layout = tab.layout.clone();
            let mut started: Vec<(PaneId, Pane)> = Vec::new();
            let mut usable = true;

            for pane in layout.panes() {
                let record = snapshot.pane(pane);
                let cwd = record
                    .and_then(|record| record.cwd.clone())
                    .or_else(|| workspace.cwd.clone());
                let command = record
                    .map(|record| record.command.clone())
                    .unwrap_or_default();
                let title = record
                    .map(|record| record.title.as_str())
                    .filter(|title| !title.is_empty())
                    .unwrap_or(SHELL_TITLE);

                match session.start_pane_as(
                    state,
                    &Starting {
                        id: pane,
                        workspace: workspace.id,
                        cwd: cwd.as_deref(),
                        command: &command,
                        title,
                        restored: &history(pane),
                    },
                ) {
                    Ok(restored) => started.push((pane, restored)),
                    Err(error) => {
                        tracing::warn!(%error, %pane, "cannot restore a pane");
                        // Its space goes to a neighbour; a tab with no
                        // neighbour left is not worth restoring. Dropping
                        // the panes already started ends their programs.
                        usable = layout.close(pane);
                        if !usable {
                            break;
                        }
                    }
                }
            }

            if !usable || started.is_empty() {
                continue;
            }
            let focus = if layout.contains(tab.focus) {
                tab.focus
            } else {
                // The pane that had the focus could not be restored, so the
                // focus goes to whatever took its place.
                match layout.panes().first() {
                    Some(first) => *first,
                    None => continue,
                }
            };

            for (id, pane) in started {
                state.panes.insert(id, pane);
            }
            let title = if tab.title.is_empty() {
                SHELL_TITLE
            } else {
                &tab.title
            };
            tabs.push(Tab::from_parts(tab.id, title, layout, focus));
        }

        if tabs.is_empty() {
            continue;
        }
        let name = if workspace.name.is_empty() {
            crate::workspace::name_for(workspace.cwd.as_deref())
        } else {
            workspace.name.clone()
        };
        state.workspaces.push(Workspace::new_with_tabs(
            workspace.id,
            name,
            workspace.cwd.clone(),
            tabs,
            workspace.active_tab,
        ));
    }
}

/// The largest identifier in a snapshot, or `fallback` when it has none.
fn highest<T: Ord>(ids: impl Iterator<Item = T>, fallback: T) -> T {
    ids.max().unwrap_or(fallback)
}

/// What to put on a restored pane's screen before its program starts.
///
/// The lines come from a snapshot, so they are treated as text and nothing
/// else: anything that could move the cursor or change a colour is dropped
/// rather than acted on.
fn restored_bytes(lines: &[String]) -> Vec<u8> {
    let mut text = String::new();
    for line in lines {
        text.extend(
            line.chars()
                .filter(|character| !character.is_control() || *character == '\t'),
        );
        text.push_str("\r\n");
    }
    text.push_str(RESTORED_DIVIDER);
    text.into_bytes()
}

/// Publish the current screen, but only when it actually changed.
fn publish(view: &watch::Sender<Arc<PaneState>>, screen: &Screen, exit: Option<PaneExit>) {
    let next = PaneState {
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
