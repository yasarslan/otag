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

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use tokio::sync::{broadcast, mpsc, watch};

use crate::frame::ScreenState;
use crate::layout::{self, Area, BORDER, Chrome, Direction, Side};
use crate::pane::{PaneExit, PaneId, PaneSize, PaneSpec};
use crate::proto::{PaneView, SessionView, TabView, WorkspaceView};
use crate::pty::{Pty, PtyEvent};
use crate::screen::Screen;
use crate::snapshot::{self, PaneSnapshot, Snapshot, TabSnapshot, WorkspaceSnapshot};
use crate::workspace::{Tab, TabId, Workspace, WorkspaceId};
use crate::{Error, Result};
use otag_agents::{AgentKind, PaneStatus, Tokens};
use otag_features::usage::Report;

/// Shortest time between two published screens.
///
/// A program can write far more often than a person can read. Updates within
/// one interval are merged, which keeps a burst of output from turning into a
/// burst of frames on the socket.
const FRAME_INTERVAL: Duration = Duration::from_millis(16);

/// How many commands may wait for a pane before a sender has to wait.
const COMMAND_QUEUE: usize = 64;

/// How many chunks of a pane's raw output are kept for clients watching it.
///
/// A client that falls this far behind is told to start again from a fresh
/// snapshot rather than being sent output with a hole in it.
const OUTPUT_QUEUE: usize = 256;

/// What a tab is called before anything better is known about it.
const SHELL_TITLE: &str = "shell";

/// How many panes that have ended stay reachable.
///
/// Enough that every client watching a pane as it ends can still be served,
/// and few enough that the screens they hold are never worth counting.
const ENDED_KEPT: usize = 8;

/// How long a hook's word on what an agent is doing outlasts the words on
/// its screen.
///
/// An agent with hooks installed reports what it is really doing; the screen
/// is read only for agents that have none, or once the hooks have gone quiet.
const HOOK_TRUST: Duration = Duration::from_secs(15);

/// The line otag draws between what a restored pane used to show and what
/// its program prints from now on.
const RESTORED_DIVIDER: &str = "\x1b[2m\u{2500}\u{2500} restored \u{2500}\u{2500}\x1b[0m\r\n";

/// What a client can ask a pane to do.
#[derive(Debug)]
enum PaneCommand {
    /// Stop this process before replacing it in the same pane.
    Stop,
    /// Write these bytes to the program's input.
    Input(Vec<u8>),
    /// The pane is now this size.
    Resize(PaneSize),
    /// Report the lines the pane has shown, for a snapshot.
    History(usize, tokio::sync::oneshot::Sender<Vec<String>>),
    /// Report what is on the screen as terminal output, together with a
    /// receiver of everything the program writes from that moment on.
    ///
    /// Both come from the pane's own task so that they fit together: a
    /// client that replayed the screen and then applied the receiver's
    /// chunks sees neither a gap nor the same output twice.
    Attach(tokio::sync::oneshot::Sender<RawAttach>),
}

/// What a client that emulates the terminal itself needs to start.
#[derive(Debug)]
pub struct RawAttach {
    /// Escape sequences that put the pane's current screen onto an empty
    /// terminal of `size`.
    pub snapshot: Vec<u8>,
    /// The size those sequences assume.
    pub size: PaneSize,
    /// Everything the program writes from now on.
    pub output: broadcast::Receiver<Arc<[u8]>>,
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

    /// What a client that emulates the terminal itself needs to start
    /// following this pane: the screen as escape sequences, and the output
    /// from that moment on.
    ///
    /// A receiver that falls far enough behind that otag has dropped output
    /// it never received is told it lagged, and the only correct answer to
    /// that is to ask for this again.
    ///
    /// A pane whose program has ended answers from its last published
    /// screen instead: that screen is everything the program ever printed,
    /// including its last line, and there is nothing more to come.
    pub async fn attach_raw(&self) -> Result<RawAttach> {
        let (attach_tx, attach_rx) = tokio::sync::oneshot::channel();
        if self.send(PaneCommand::Attach(attach_tx)).await.is_ok()
            && let Ok(attach) = attach_rx.await
        {
            return Ok(attach);
        }
        self.last_attach()
    }

    /// What a client needs in order to show a pane that is already over.
    ///
    /// The pane's task is gone, so nobody can build this from the emulator
    /// any more; the last state the task published is the whole truth, and
    /// an output channel with no sender tells the stream straight away that
    /// there is nothing to follow.
    fn last_attach(&self) -> Result<RawAttach> {
        let state = self.snapshot();
        if state.exit.is_none() {
            // The task is unreachable without having reported an end, which
            // is not a pane a client can be given.
            return Err(Error::PaneGone { id: Some(self.id) });
        }

        let (sender, output) = broadcast::channel(1);
        drop(sender);
        Ok(RawAttach {
            snapshot: crate::replay::snapshot(&state.screen),
            size: state.screen.size(),
            output,
        })
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
    /// The directory it is working in, when otag knows.
    pub cwd: Option<PathBuf>,
    /// What it is running.
    pub title: String,
    /// The agent otag has noticed in it.
    pub agent: Option<AgentKind>,
    /// What that agent is doing.
    pub status: PaneStatus,
    /// The agent session otag would resume.
    pub session: Option<String>,
    /// The word the user gave it for the job it is doing.
    pub nickname: String,
    /// The first thing the user asked it, when otag knows.
    pub first_prompt: Option<String>,
    /// What the agent in it has spent.
    pub tokens: Tokens,
    /// How large the model's context is, when otag knows.
    pub context_window: Option<u64>,
    /// How its program ended, if it has.
    pub exit: Option<PaneExit>,
}

/// What otag knows about one pane, for the task watching the agents.
#[derive(Debug, Clone)]
pub struct AgentPane {
    /// Which pane this is.
    pub id: PaneId,
    /// The program on its pseudo-terminal, which an agent runs below.
    pub pid: Option<u32>,
    /// Where it is working.
    pub cwd: Option<PathBuf>,
    /// When it started.
    pub started_at: SystemTime,
    /// The agent otag has noticed in it.
    pub agent: Option<AgentKind>,
    /// What that agent is doing.
    pub status: PaneStatus,
    /// The agent session otag would resume.
    pub session_id: Option<String>,
    /// The file that session is kept in.
    pub session_path: Option<PathBuf>,
    /// What the agent in it has spent, as far as otag has read.
    pub tokens: Tokens,
    /// Whether a guess from the screen may set the status.
    pub trusts_the_screen: bool,
    /// What is on the screen, for the guess.
    pub screen: String,
}

/// Everything needed to start one pane.
struct Starting<'a> {
    id: PaneId,
    workspace: WorkspaceId,
    cwd: Option<&'a Path>,
    /// What to run, empty for the user's shell.
    command: &'a [String],
    title: &'a str,
    /// The nickname it is being restored with, empty for a new pane.
    nickname: &'a str,
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
    /// Identifier of the program on its pseudo-terminal, for finding the
    /// agent a user started inside the shell.
    pid: Option<u32>,
    /// When it started, so that a session older than the pane is not taken
    /// for the pane's own.
    started_at: SystemTime,
    /// The agent running in it, once otag has noticed one.
    agent: Option<AgentKind>,
    /// What that agent is doing.
    status: PaneStatus,
    /// The agent session otag would resume.
    session_id: Option<String>,
    /// The file that session is kept in, for reading what it has spent.
    session_path: Option<PathBuf>,
    /// The word the user gave it for the job it is doing.
    ///
    /// Kept here rather than in a client's preferences so that it survives a
    /// restart and every client shows the same one. It goes into the
    /// snapshot with everything else about the pane.
    nickname: String,
    /// The first thing the user asked it, when otag knows.
    ///
    /// From the prompt a client started it with, or from the agent's own
    /// transcript for a session otag did not start. Only the first one is
    /// kept: it is what the session is *for*, and a name that changed every
    /// turn would not be a name.
    first_prompt: Option<String>,
    /// What the agent in it has spent, as far as otag has read.
    tokens: Tokens,
    /// When a hook last said what the agent is doing.
    ///
    /// Hooks report what the agent is actually doing; the words on its
    /// screen are a guess. While hooks are arriving, the guess is not used.
    last_hook: Option<Instant>,
    /// How large the model's context is, when the agent's session file says.
    ///
    /// Without it a count of context tokens cannot be shown as a share of
    /// anything, so otag shows the count alone rather than inventing a
    /// denominator.
    context_window: Option<u64>,
    /// The size a client that draws this pane in a window of its own has
    /// asked for.
    ///
    /// otag's own layout decides how large a pane is *inside* otag. A client
    /// that draws the pane somewhere else — the macOS app, which gives each
    /// pane its own view — knows better, so once such a client has said how
    /// large the pane is on its screen, the layout stops resizing it.
    size_override: Option<PaneSize>,
}

impl Pane {
    /// Whether a guess from the pane's screen may set its status.
    fn trusts_the_screen(&self) -> bool {
        self.last_hook.is_none_or(|at| at.elapsed() > HOOK_TRUST)
    }
}

/// Everything one daemon owns.
#[derive(Debug)]
pub struct Session {
    team_operations: tokio::sync::Mutex<()>,
    /// What every new pane runs, and the environment it runs in.
    template: PaneSpec,
    /// Bumped on every structural change, so clients know to ask for a view.
    changes: watch::Sender<u64>,
    /// Coalesced screen changes, for saving history without idle polling.
    content_changes: watch::Sender<u64>,
    state: Mutex<State>,
}

#[derive(Debug)]
struct State {
    teams: Vec<crate::team::Team>,
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
    /// Handles on the panes that have ended, newest last.
    ///
    /// A client can ask to watch a pane at any moment, including just after
    /// its program printed its last line and ended. Looking the pane up in
    /// `panes` then finds nothing, and the client would be told the pane is
    /// gone instead of being shown what the program left on the screen. The
    /// handle is kept so that attaching late still works; it holds no
    /// pseudo-terminal and no process, only the last screen.
    ended: VecDeque<(PaneId, PaneHandle)>,
    /// How much of the agents' limits is used, as the daemon last worked it
    /// out. The session does not compute this; it carries it to the clients
    /// with everything else they draw.
    usage: Report,
    /// What `COLORFGBG` should say in a pane started from now on, once a
    /// client has said which way round its terminal is.
    ///
    /// Not set until a client says, because guessing would be worse than
    /// saying nothing: a program that reads the variable would be told
    /// something otag made up.
    colorfgbg: Option<String>,
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
    /// program is expecting. A pane a client draws in a window of its own
    /// keeps the size that client asked for.
    fn pending_resizes(&self) -> Vec<(PaneHandle, PaneSize)> {
        self.areas()
            .into_iter()
            .filter_map(|(id, area)| {
                let pane = self.panes.get(&id)?;
                if pane.size_override.is_some() {
                    return None;
                }
                let size = area.inset(BORDER).size();
                (pane.handle.snapshot().screen.size() != size).then(|| (pane.handle.clone(), size))
            })
            .collect()
    }

    /// What a group of panes is doing, taken from the one that most wants
    /// the user.
    fn status_of(&self, panes: &[PaneId]) -> PaneStatus {
        panes
            .iter()
            .filter_map(|pane| self.panes.get(pane))
            .map(|pane| pane.status)
            .max()
            .unwrap_or_default()
    }

    /// Names of the workspaces, for giving a new one a name of its own.
    fn names(&self) -> Vec<String> {
        self.workspaces
            .iter()
            .map(|workspace| workspace.name().to_owned())
            .collect()
    }

    /// The workspace already open on `cwd`'s project, if there is one.
    ///
    /// Compared by [`crate::workspace::same_project`], so a path spelled
    /// another way, a path through a symbolic link and a git worktree of a
    /// repository all find the entry the user already has.
    fn workspace_for_project(&self, cwd: &Path) -> Option<WorkspaceId> {
        let wanted = crate::workspace::project_root(cwd);
        self.workspaces
            .iter()
            .find(|workspace| {
                workspace
                    .cwd()
                    .is_some_and(|open| crate::workspace::project_root(open) == wanted)
            })
            .map(Workspace::id)
    }

    /// Where the workspace with this identifier is, if it is still here.
    fn position_of(&self, workspace: WorkspaceId) -> Option<usize> {
        self.workspaces
            .iter()
            .position(|candidate| candidate.id() == workspace)
    }
}

impl Session {
    /// Serialize workflow transitions, including process and filesystem I/O.
    pub async fn team_operation(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.team_operations.lock().await
    }
    /// Surface tool-event evidence of a read-only role attempting an edit.
    pub async fn note_read_only_edit(self: &Arc<Self>, pane: PaneId) {
        let _operation = self.team_operation().await;
        let mut teams = self.teams();
        let mut changed = false;
        for team in &mut teams {
            if let Some(index) = team
                .members
                .iter()
                .position(|m| m.pane == pane && !m.role.editable())
                && !team.violations.contains(&index)
            {
                team.violations.push(index);
                team.automatic = false;
                changed = true;
            }
        }
        if changed {
            self.set_teams(teams).await;
        }
    }
    /// Durable team state, copied without holding a lock across I/O.
    pub fn teams(&self) -> Vec<crate::team::Team> {
        self.state
            .lock()
            .map(|s| s.teams.clone())
            .unwrap_or_default()
    }

    /// Publish a structural workflow change and persist it with the panes.
    pub async fn set_teams(self: &Arc<Self>, teams: Vec<crate::team::Team>) {
        if let Ok(mut state) = self.lock() {
            state.teams = teams;
        }
        self.commit(true).await;
    }

    /// Restart a managed role in its stable pane after stopping its old process tree.
    pub async fn restart_role(
        self: &Arc<Self>,
        id: PaneId,
        command: &[String],
        title: &str,
    ) -> Result<()> {
        let previous = self.pane(id).ok_or(Error::PaneGone { id: Some(id) })?;
        let history = previous
            .history(snapshot::HISTORY_LINES)
            .await
            .unwrap_or_default();
        let pid = self.lock()?.panes.get(&id).and_then(|p| p.pid);
        if let Some(pid) = pid {
            tokio::task::spawn_blocking(move || crate::process::terminate(&[pid as i32]))
                .await
                .map_err(|_| Error::PaneGone { id: Some(id) })?
                .map_err(|_| Error::PaneGone { id: Some(id) })?;
        }
        let old = {
            let mut state = self.lock()?;
            let from = state
                .panes
                .get(&id)
                .ok_or(Error::PaneGone { id: Some(id) })?;
            let workspace = from.workspace;
            let cwd = from.cwd.clone();
            let nickname = from.nickname.clone();
            let pane = self.start_pane_as(
                &state,
                &Starting {
                    id,
                    workspace,
                    cwd: cwd.as_deref(),
                    command,
                    title,
                    nickname: &nickname,
                    restored: &history,
                },
            )?;
            state.panes.insert(id, pane)
        };
        if let Some(old) = old {
            let _ = old.handle.send(PaneCommand::Stop).await;
        }
        self.commit(true).await;
        Ok(())
    }
    /// A daemon with no panes, whose panes will run `template`.
    #[must_use]
    pub fn new(template: PaneSpec) -> Arc<Self> {
        Arc::new(Self {
            team_operations: tokio::sync::Mutex::new(()),
            template,
            changes: watch::Sender::new(0),
            content_changes: watch::Sender::new(0),
            state: Mutex::new(State {
                teams: Vec::new(),
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
                ended: VecDeque::new(),
                usage: Report::default(),
                colorfgbg: None,
            }),
        })
    }

    /// A receiver that wakes on every structural change.
    #[must_use]
    pub fn changes(&self) -> watch::Receiver<u64> {
        self.changes.subscribe()
    }

    /// A receiver that wakes when any pane publishes a screen or exits.
    #[must_use]
    pub fn content_changes(&self) -> watch::Receiver<u64> {
        self.content_changes.subscribe()
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

    /// Divide a pane and start a program of otag's choosing beside it.
    ///
    /// This is how a handoff opens the agent that takes over: beside the pane
    /// it is taking over from, in the same directory, so the user sees both
    /// at once and the one that ran out of quota stays where it is.
    pub async fn split_running(
        self: &Arc<Self>,
        beside: PaneId,
        direction: Direction,
        command: &[String],
        title: &str,
    ) -> Result<PaneId> {
        let started = {
            let mut state = self.lock()?;
            let Some(from) = state.panes.get(&beside) else {
                return Err(Error::PaneGone { id: Some(beside) });
            };
            let workspace = from.workspace;
            let cwd = from.cwd.clone();

            // Seed the name before publishing the pane: its transcript starts
            // with otag's handoff instruction rather than the user's task.
            let name = from.first_prompt.as_deref().unwrap_or(&from.title);
            let first_prompt = format!("{name} ({title})");
            let nickname = from.nickname.clone();

            let pane =
                self.start_pane_running(&mut state, workspace, cwd.as_deref(), command, title)?;
            if let Some(started) = state.panes.get_mut(&pane) {
                started.first_prompt = Some(first_prompt);
                started.nickname = nickname;
            }
            // The split happens next to the pane being handed over from, so
            // the focus is moved there first.
            let placed = state.tab_mut().is_some_and(|tab| {
                // The split happens at the focus, so the pane being handed
                // over from becomes the focus first. A pane in another tab
                // cannot be split beside, and says so.
                tab.focus_pane(beside) && tab.split(direction, pane)
            });
            if !placed {
                state.panes.remove(&pane);
                return Err(Error::NoPanes);
            }
            pane
        };
        self.commit(true).await;
        Ok(started)
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

    /// Start a session of its own: a workspace with one tab and one pane
    /// running `command`, shown straight away.
    ///
    /// This is what a client's "new session" does. An empty command runs the
    /// user's shell, which is the same as [`Self::new_workspace`]; a command
    /// starts an agent. Returns the pane, because the client that asked for
    /// it wants to attach to it.
    ///
    /// A session started in a project the user already has open joins that
    /// project as a tab of its own, because that is what the user asked for:
    /// another session in this project, not another project. Only
    /// [`Self::new_workspace`] — someone asking for an entry of its own —
    /// makes a second one for the same directory.
    pub async fn start_session(
        self: &Arc<Self>,
        cwd: Option<&Path>,
        command: &[String],
        title: &str,
    ) -> Result<PaneId> {
        let pane = {
            let mut state = self.lock()?;
            // Without this, a second session in a folder whose name was
            // already taken made an entry called "<project> 2" — which is
            // what that numbering is for when two *different* checkouts
            // share a name, and plainly wrong for the same one.
            match cwd.and_then(|cwd| state.workspace_for_project(cwd)) {
                Some(open) => self.add_tab_to(&mut state, open, cwd, command, title)?,
                None => self.start_workspace_running(&mut state, cwd, command, title)?,
            }
        };
        self.commit(true).await;
        Ok(pane)
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

    /// Give one pane the size the client drawing it has.
    ///
    /// This is for a client that draws the pane in a window of its own
    /// rather than inside otag's layout: from here on the layout leaves that
    /// pane's size alone, because the client is the only one that knows it.
    pub async fn resize_pane_to(&self, id: PaneId, size: PaneSize) -> Result<()> {
        let handle = {
            let mut state = self.lock()?;
            let pane = state
                .panes
                .get_mut(&id)
                .ok_or(Error::PaneGone { id: Some(id) })?;
            pane.size_override = Some(size);
            pane.handle.clone()
        };
        handle.resize(size).await
    }

    /// The pane with this identifier, if the daemon has it.
    #[must_use]
    pub fn pane(&self, id: PaneId) -> Option<PaneHandle> {
        let state = self.state.lock().ok()?;
        state.panes.get(&id).map(|pane| pane.handle.clone())
    }

    /// A pane whose program has ended, if it is still remembered.
    ///
    /// Only for showing what is over: the pane is out of the layout, nothing
    /// can be typed into it, and the one thing it can still answer is what
    /// its program left on the screen and how it ended.
    #[must_use]
    pub fn ended_pane(&self, id: PaneId) -> Option<PaneHandle> {
        let state = self.state.lock().ok()?;
        state
            .ended
            .iter()
            .rev()
            .find(|(ended, _)| *ended == id)
            .map(|(_, handle)| handle.clone())
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
        self.view_with_panes().0
    }

    /// Everything a client has to draw, and a handle on every pane it shows.
    ///
    /// Both come from the same look at the session on purpose. A program can
    /// end at any moment, and a client that took the view first and then
    /// asked for its panes could find one already gone — leaving it with
    /// nowhere to read that program's last output from. A handle taken here
    /// keeps the pane's final screen reachable however late the client gets
    /// round to sending it.
    #[must_use]
    pub fn view_with_panes(&self) -> (SessionView, HashMap<PaneId, PaneHandle>) {
        let Ok(state) = self.state.lock() else {
            // A poisoned lock means a holder panicked. An empty view says
            // there is nothing to draw, which is better than a panic in
            // every client that asks.
            return (SessionView::default(), HashMap::new());
        };

        let chrome = state.chrome();
        let mut panes = Vec::new();
        let mut handles = HashMap::new();
        for (id, area) in state.areas() {
            let Some(pane) = state.panes.get(&id) else {
                continue;
            };
            let snapshot = pane.handle.snapshot();
            panes.push(PaneView {
                id,
                area,
                title: pane.title.clone(),
                agent: pane.agent,
                status: pane.status,
                frame: Some(snapshot.screen.to_frame()),
                exit: snapshot.exit.clone(),
            });
            handles.insert(id, pane.handle.clone());
        }

        let view = SessionView {
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
                    status: state.status_of(&workspace.panes()),
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
                            status: state.status_of(&tab.panes()),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            active_tab: state.workspace().map_or(0, Workspace::active),
            panes,
            focus: state.focus(),
            usage: state.usage.clone(),
        };

        (view, handles)
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
                    agent: pane.agent,
                    status: pane.status,
                    session: pane.session_id.clone(),
                    nickname: pane.nickname.clone(),
                    first_prompt: pane.first_prompt.clone(),
                    tokens: pane.tokens,
                    context_window: pane.context_window,
                    workspace: state
                        .workspaces
                        .iter()
                        .find(|workspace| workspace.id() == pane.workspace)
                        .map_or_else(String::new, |workspace| workspace.name().to_owned()),
                    cwd: pane.cwd.clone(),
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

    /// What otag knows about the panes, for whatever is watching them.
    ///
    /// One call rather than one per pane, so that a scan of the processes or
    /// of the agents' session files sees the panes as they were at one
    /// moment.
    #[must_use]
    pub fn agent_panes(&self) -> Vec<AgentPane> {
        let Ok(state) = self.state.lock() else {
            return Vec::new();
        };
        state
            .panes
            .values()
            .map(|pane| AgentPane {
                id: pane.handle.id(),
                pid: pane.pid,
                cwd: pane.cwd.clone(),
                started_at: pane.started_at,
                agent: pane.agent,
                status: pane.status,
                session_id: pane.session_id.clone(),
                session_path: pane.session_path.clone(),
                tokens: pane.tokens,
                trusts_the_screen: pane.trusts_the_screen(),
                screen: pane.handle.snapshot().screen.text(),
            })
            .collect()
    }

    /// Record that a pane is running an agent.
    ///
    /// The pane's title becomes the agent's name, so that a tab shows what
    /// is in it rather than what started it.
    pub async fn set_agent(self: &Arc<Self>, pane: PaneId, agent: Option<AgentKind>) {
        let changed = self.change(pane, |pane| {
            if pane.agent == agent {
                return false;
            }
            pane.agent = agent;
            pane.title = agent.map_or_else(|| SHELL_TITLE.to_owned(), |agent| agent.to_string());
            if agent.is_none() {
                // Whatever the agent was doing, it is not doing it now.
                pane.status = PaneStatus::default();
                pane.session_id = None;
            }
            true
        });
        self.commit(changed).await;
    }

    /// Record what the agent in a pane is doing.
    ///
    /// `from_hook` says the agent reported this itself, which is worth more
    /// than a guess from its screen and silences the guessing for a while.
    pub async fn set_status(
        self: &Arc<Self>,
        pane: PaneId,
        status: PaneStatus,
        from_hook: bool,
    ) -> bool {
        let changed = self.change(pane, |pane| {
            if from_hook {
                pane.last_hook = Some(Instant::now());
            } else if !pane.trusts_the_screen() && !status.wants_attention() {
                // The agent is reporting for itself; its screen is not
                // needed and may be a turn behind.
                //
                // Except when the screen is asking for the user: that is the
                // screen as it is now, and an agent without hooks has no
                // other way to say it. Leaving a question unreported because
                // the agent happened to report a minute ago is the one
                // mistake here that costs the user something.
                return false;
            }
            let changed = pane.status != status;
            pane.status = status;
            changed
        });
        self.commit(changed).await;
        changed
    }

    /// Record which of an agent's sessions a pane is running.
    pub async fn set_session_id(self: &Arc<Self>, pane: PaneId, session_id: String) -> bool {
        let changed = self.change(pane, |pane| {
            if pane.session_id.as_deref() == Some(session_id.as_str()) {
                return false;
            }
            pane.session_id = Some(session_id.clone());
            true
        });
        self.commit(changed).await;
        changed
    }

    /// Record where the agent's session file is, for reading what it has
    /// spent.
    pub async fn set_session_path(self: &Arc<Self>, pane: PaneId, path: PathBuf) {
        let changed = self.change(pane, |pane| {
            if pane.session_path.as_deref() == Some(path.as_path()) {
                return false;
            }
            pane.session_path = Some(path.clone());
            true
        });
        // Nothing a client can see changed.
        let _ = changed;
    }

    /// Record what the agent in a pane has spent.
    pub async fn set_tokens(self: &Arc<Self>, pane: PaneId, tokens: Tokens) {
        let changed = self.change(pane, |pane| {
            if pane.tokens == tokens {
                return false;
            }
            pane.tokens = tokens;
            true
        });
        // Only the usage widget shows this, and it is published with the
        // report rather than per pane.
        let _ = changed;
    }

    /// Carry a new usage report to the clients.
    ///
    /// Reports that say the same thing are dropped, because publishing one
    /// makes every client redraw.
    pub async fn set_usage(self: &Arc<Self>, report: Report) {
        let changed = match self.lock() {
            Ok(mut state) => {
                if state.usage == report {
                    false
                } else {
                    state.usage = report;
                    true
                }
            }
            Err(_) => false,
        };
        self.commit(changed).await;
    }

    /// How much of the agents' limits is used, as the daemon last worked it
    /// out.
    #[must_use]
    pub fn usage(&self) -> Report {
        self.lock()
            .map(|state| state.usage.clone())
            .unwrap_or_default()
    }

    /// How long a nickname may be.
    ///
    /// A nickname is a word, not a sentence: it has to fit under a name in a
    /// sidebar row without pushing everything else about.
    pub const NICKNAME_LIMIT: usize = 40;

    /// Give a session a nickname, or clear it with an empty one.
    pub async fn set_nickname(self: &Arc<Self>, pane: PaneId, nickname: &str) {
        let wanted: String = nickname.trim().chars().take(Self::NICKNAME_LIMIT).collect();

        let changed = self.change(pane, |pane| {
            if pane.nickname == wanted {
                return false;
            }
            pane.nickname = wanted.clone();
            true
        });
        self.commit(changed).await;
    }

    /// How long a first prompt may be when it is carried as a name.
    ///
    /// A name is one short line, and a prompt can be a pasted file. Clipping
    /// here rather than in the client means nothing large travels over the
    /// socket on every pane report.
    pub const FIRST_PROMPT_LIMIT: usize = 400;

    /// Record the first thing the user asked a pane.
    ///
    /// Only the first: it is what the session is *for*, and a name that
    /// changed every turn would not be a name. So a prompt arriving for a
    /// pane that already has one changes nothing, which is also what lets
    /// the transcript reader offer one on every pass without cost.
    pub async fn set_first_prompt(self: &Arc<Self>, pane: PaneId, prompt: &str) {
        let wanted = prompt.trim();
        if wanted.is_empty() {
            return;
        }
        let wanted: String = wanted.chars().take(Self::FIRST_PROMPT_LIMIT).collect();

        let changed = self.change(pane, |pane| {
            if pane.first_prompt.is_some() {
                return false;
            }
            pane.first_prompt = Some(wanted.clone());
            true
        });
        self.commit(changed).await;
    }

    /// Say whether the clients are drawing their panes on dark paper, so
    /// that a pane started from now on can tell its program.
    ///
    /// Panes already running are not touched: a process's environment cannot
    /// be changed from outside it. A running agent learns the same thing by
    /// asking its pane with `OSC 11`.
    pub fn set_terminal_appearance(&self, dark: bool) {
        if let Ok(mut state) = self.state.lock() {
            state.colorfgbg = Some(crate::pane::colorfgbg(dark).to_owned());
        }
    }

    /// Record how large the context of the model in a pane is.
    pub async fn set_context_window(self: &Arc<Self>, pane: PaneId, window: u64) {
        let changed = self.change(pane, |pane| {
            let changed = pane.context_window != Some(window);
            pane.context_window = Some(window);
            changed
        });
        self.commit(changed).await;
    }

    /// What each pane has spent, for attributing usage to an agent.
    #[must_use]
    pub fn pane_usage(&self) -> Vec<(PaneId, Option<AgentKind>, Tokens)> {
        let Ok(state) = self.state.lock() else {
            return Vec::new();
        };
        state
            .panes
            .values()
            .map(|pane| (pane.handle.id(), pane.agent, pane.tokens))
            .collect()
    }

    /// Record the command that would bring a pane back as it is now.
    ///
    /// A pane running an agent comes back by resuming that agent's session,
    /// which is what makes a restored agent carry on rather than start over.
    pub async fn set_restore_command(self: &Arc<Self>, pane: PaneId, command: Vec<String>) {
        let changed = self.change(pane, |pane| {
            if pane.command == command {
                return false;
            }
            pane.command = command.clone();
            true
        });
        // Nothing a client can see changed, but the next snapshot differs.
        self.commit(changed).await;
    }

    /// Change one pane, reporting whether anything did change.
    fn change(&self, pane: PaneId, edit: impl FnOnce(&mut Pane) -> bool) -> bool {
        self.lock()
            .ok()
            .and_then(|mut state| state.panes.get_mut(&pane).map(edit))
            .unwrap_or(false)
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
            teams: state.teams.clone(),
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
                    nickname: pane.nickname.clone(),
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
            state.teams = snapshot.teams.clone();
            for team in &mut state.teams {
                team.running = false;
                team.queued = false;
                team.write_root = None;
            }
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
    async fn close_pane(self: &Arc<Self>, handle: &PaneHandle, exit: Option<PaneExit>) {
        let id = handle.id();
        {
            let Ok(mut state) = self.lock() else { return };
            if state
                .panes
                .get(&id)
                .is_none_or(|p| !p.handle.commands.same_channel(&handle.commands))
            {
                return;
            }
            if state
                .teams
                .iter()
                .any(|t| t.members.iter().any(|m| m.pane == id))
            {
                if let Some(pane) = state.panes.get_mut(&id) {
                    pane.pid = None;
                    pane.status = if exit.as_ref().is_some_and(PaneExit::success) {
                        PaneStatus::Done
                    } else {
                        PaneStatus::Error
                    };
                }
                drop(state);
                self.changes.send_modify(|n| *n = n.wrapping_add(1));
                return;
            }
            let Some(pane) = state.panes.remove(&id) else {
                return;
            };
            // A client may still be about to ask for this pane: it is kept
            // reachable so that the last thing the program printed, and the
            // news that it ended, are still there to be sent.
            state.ended.push_back((id, pane.handle));
            while state.ended.len() > ENDED_KEPT {
                state.ended.pop_front();
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
        self.start_workspace_running(state, cwd, &[], SHELL_TITLE)?;
        Ok(())
    }

    /// Start a workspace whose one pane runs a command of its own.
    fn start_workspace_running(
        self: &Arc<Self>,
        state: &mut State,
        cwd: Option<&Path>,
        command: &[String],
        title: &str,
    ) -> Result<PaneId> {
        let id = state.next_workspace;
        state.next_workspace = id.next();
        let name = crate::workspace::unique_name(&crate::workspace::name_for(cwd), &state.names());

        let pane = self.start_pane_running(state, id, cwd, command, title)?;
        let tab_id = state.next_tab;
        state.next_tab = tab_id.next();

        let workspace = Workspace::new(
            id,
            name,
            cwd.map(Path::to_path_buf),
            Tab::new(tab_id, title, pane),
        );
        state.workspaces.push(workspace);
        state.active = state.workspaces.len().saturating_sub(1);
        Ok(pane)
    }

    /// Add a tab running `command` to a workspace that is already open, and
    /// show it.
    fn add_tab_to(
        self: &Arc<Self>,
        state: &mut State,
        workspace: WorkspaceId,
        cwd: Option<&Path>,
        command: &[String],
        title: &str,
    ) -> Result<PaneId> {
        let pane = self.start_pane_running(state, workspace, cwd, command, title)?;
        let tab_id = state.next_tab;
        state.next_tab = tab_id.next();

        let Some(position) = state.position_of(workspace) else {
            // It was there a moment ago; the pane is started and the caller
            // still gets it, so nothing is left running unowned.
            return Ok(pane);
        };
        state.workspaces[position].add_tab(Tab::new(tab_id, title, pane));
        state.active = position;
        Ok(pane)
    }

    /// Start a pane, and the task that notices when its program ends.
    fn start_pane(
        self: &Arc<Self>,
        state: &mut State,
        workspace: WorkspaceId,
        cwd: Option<&Path>,
    ) -> Result<PaneId> {
        self.start_pane_running(state, workspace, cwd, &[], SHELL_TITLE)
    }

    /// Start a pane running a command of its own, and put it in the session.
    ///
    /// An empty command is the user's shell, which is what every pane the
    /// user opens runs; a handoff passes the command that starts an agent.
    fn start_pane_running(
        self: &Arc<Self>,
        state: &mut State,
        workspace: WorkspaceId,
        cwd: Option<&Path>,
        command: &[String],
        title: &str,
    ) -> Result<PaneId> {
        let id = state.next_pane;
        let pane = self.start_pane_as(
            state,
            &Starting {
                id,
                workspace,
                cwd,
                command,
                title,
                // A pane nobody has nicknamed yet. Only a restore brings
                // one in, and the user gives the rest.
                nickname: "",
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

        // What the client said about its own paper, so a program that reads
        // `COLORFGBG` rather than asking the terminal is told the truth from
        // its first line.
        let spec = match &state.colorfgbg {
            Some(value) => spec.with_env(crate::pane::COLORFGBG, value),
            None => spec,
        };

        // A pane is started at the size of the whole content area and told
        // its real size as soon as the layout is known, which is one resize
        // before the program has drawn anything.
        let size = state.chrome().content.inset(BORDER).size();
        let (handle, pid) = spawn_pane(starting.id, &spec, size, starting.restored)?;
        self.reap(handle.clone());

        Ok(Pane {
            handle,
            title: starting.title.to_owned(),
            workspace: starting.workspace,
            cwd: starting.cwd.map(Path::to_path_buf),
            command: starting.command.to_vec(),
            pid,
            started_at: SystemTime::now(),
            agent: None,
            status: PaneStatus::default(),
            session_id: None,
            session_path: None,
            nickname: starting.nickname.to_owned(),
            first_prompt: None,
            tokens: Tokens::default(),
            last_hook: None,
            context_window: None,
            size_override: None,
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
        self.content_changes
            .send_modify(|revision| *revision = revision.wrapping_add(1));
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
                session
                    .content_changes
                    .send_modify(|revision| *revision = revision.wrapping_add(1));
            };
            session.close_pane(&handle, exit).await;
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
) -> Result<(PaneHandle, Option<u32>)> {
    let (pty, events) = Pty::spawn(id, spec, size)?;
    let pid = pty.pid();
    let mut screen = Screen::new(size);
    if !restored.is_empty() {
        screen.process(&restored_bytes(restored));
    }

    let (view_tx, view_rx) = watch::channel(Arc::new(PaneState {
        screen: screen.state(),
        exit: None,
    }));
    let (commands_tx, commands_rx) = mpsc::channel(COMMAND_QUEUE);
    // The pane's own task owns the only sender: once it ends, everything
    // watching the pane's output learns that no more is coming.
    let (output_tx, _) = broadcast::channel(OUTPUT_QUEUE);

    tokio::spawn(run_pane(
        id,
        pty,
        screen,
        events,
        commands_rx,
        view_tx,
        output_tx,
    ));

    Ok((
        PaneHandle {
            id,
            commands: commands_tx,
            view: view_rx,
        },
        pid,
    ))
}

/// Drive one pane until its program ends or every client is gone.
async fn run_pane(
    id: PaneId,
    mut pty: Pty,
    mut screen: Screen,
    mut events: mpsc::Receiver<PtyEvent>,
    mut commands: mpsc::Receiver<PaneCommand>,
    view: watch::Sender<Arc<PaneState>>,
    output: broadcast::Sender<Arc<[u8]>>,
) {
    let mut ticker = tokio::time::interval(FRAME_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut dirty = false;

    loop {
        tokio::select! {
            event = events.recv() => match event {
                Some(PtyEvent::Output(bytes)) => {
                    // Clients that emulate the terminal themselves get the
                    // bytes as they are, before anything is made of them. An
                    // error here only means nobody is watching.
                    let _ = output.send(Arc::from(bytes.as_slice()));

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
                    // The exit comes after every chunk of output on the same
                    // channel, so the screen published here already shows
                    // everything the program printed, including whatever it
                    // printed last. Nothing is left for the frame ticker,
                    // which stops with this task.
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
                Some(PaneCommand::Stop) => { pty.kill(); return; }
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
                Some(PaneCommand::Attach(answer)) => {
                    // Built here, in the task that owns both the screen and
                    // the output, so that the snapshot and the subscription
                    // describe the same moment.
                    let _ = answer.send(RawAttach {
                        snapshot: crate::replay::snapshot(&screen.state()),
                        size: screen.state().size(),
                        output: output.subscribe(),
                    });
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
                let mut command = record
                    .map(|record| record.command.clone())
                    .unwrap_or_default();
                if state
                    .teams
                    .iter()
                    .any(|t| t.members.iter().any(|m| m.pane == pane))
                {
                    command = vec!["/bin/sleep".into(), "2147483647".into()];
                }
                let title = record
                    .map(|record| record.title.as_str())
                    .filter(|title| !title.is_empty())
                    .unwrap_or(SHELL_TITLE);
                let nickname = record.map_or("", |record| record.nickname.as_str());

                match session.start_pane_as(
                    state,
                    &Starting {
                        id: pane,
                        workspace: workspace.id,
                        cwd: cwd.as_deref(),
                        command: &command,
                        title,
                        nickname,
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
