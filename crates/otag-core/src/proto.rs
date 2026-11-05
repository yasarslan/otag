//! The otag socket protocol.
//!
//! Clients and the daemon exchange newline-delimited JSON over a Unix domain
//! socket: one [`Request`] object per line, answered by one [`Response`] object
//! per line.
//!
//! The protocol is designed to grow. Both enums are internally tagged and carry
//! a catch-all variant, so a client and a daemon of different versions degrade
//! to a clear error instead of a parse failure, and unknown object fields are
//! ignored rather than rejected.
//!
//! There are two ways to watch a session. A client that lets the daemon draw
//! asks for [`Request::Attach`] and receives views and frames; a client with
//! a terminal of its own — the macOS app — asks for
//! [`Request::AttachPane`] and receives the pane's bytes. Either way the
//! daemon owns the pane, and both kinds of client may watch the same one at
//! the same time.

use std::path::PathBuf;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::frame::ScreenFrame;
use crate::layout::{Area, Direction, Side};
use crate::pane::{PaneExit, PaneId, PaneSize};
use crate::workspace::{TabId, WorkspaceId};
use otag_agents::approval::Answer;
use otag_agents::{AgentKind, PaneStatus, Tokens};
use otag_features::inbox::{Ask, AskId};
use otag_features::usage::Report;

/// A command sent from a client to the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    /// Check that the daemon is alive. Answered with [`Response::Pong`].
    Ping,
    /// Ask the daemon for its version. Answered with [`Response::Version`].
    Version,
    /// Ask the daemon to shut down. Answered with [`Response::Ok`] before the
    /// daemon stops accepting new connections.
    Shutdown,
    /// List the panes the daemon is running. Answered with
    /// [`Response::Panes`].
    ListPanes,
    /// Ask how much of the agents' limits is used. Answered with
    /// [`Response::Usage`].
    Usage,
    /// Attach to the session, starting a workspace and a pane if the daemon
    /// has none.
    ///
    /// Answered with [`Response::View`], after which the daemon pushes a
    /// [`Response::Frame`] whenever a visible pane's screen changes and a new
    /// [`Response::View`] whenever the layout changes, until the client sends
    /// [`Request::Detach`] or the connection ends.
    Attach {
        /// Size of the terminal the client is drawing into.
        size: PaneSize,
        /// Directory a newly started workspace should run in.
        #[serde(default)]
        cwd: Option<PathBuf>,
    },
    /// Write these bytes to the focused pane's input.
    ///
    /// Not answered: a key press is not worth a round trip, and the result
    /// is visible in the next frame.
    Input {
        /// Exactly what the program should receive.
        bytes: Bytes,
    },
    /// The client's terminal changed size.
    ///
    /// Not answered; the next view carries the new geometry.
    Resize {
        /// New size of the client's terminal.
        size: PaneSize,
    },
    /// Divide the focused pane and start a pane in the new half.
    Split {
        /// How the two halves are arranged.
        direction: Direction,
    },
    /// Add a tab with one pane to the active workspace, and show it.
    NewTab,
    /// Show the tab at this position in the active workspace.
    SelectTab {
        /// Position of the tab, counting from zero.
        index: usize,
    },
    /// Add a workspace with one tab and one pane, and show it.
    NewWorkspace {
        /// Directory the workspace's panes start in.
        #[serde(default)]
        cwd: Option<PathBuf>,
    },
    /// Show the workspace at this position.
    SelectWorkspace {
        /// Position of the workspace, counting from zero.
        index: usize,
    },
    /// Focus this pane, if the active tab holds it.
    FocusPane {
        /// The pane to type into.
        pane: PaneId,
    },
    /// Focus the pane on this side of the focused one.
    FocusSide {
        /// Which way to look.
        side: Side,
    },
    /// Focus the next pane in the active tab, wrapping around.
    FocusNext,
    /// Move the focused pane's edge on this side outwards.
    ResizePane {
        /// Which edge to move.
        side: Side,
        /// How far to move it, in cells.
        cells: u16,
    },
    /// Show or hide the workspace sidebar.
    ToggleSidebar,
    /// Carry the work in a pane over to another agent, or to another model
    /// of the same agent. Answered with [`Response::HandoffDone`].
    Handoff {
        /// The pane whose work is being handed over.
        pane: PaneId,
        /// What to do with it.
        handoff: Handoff,
    },
    /// What an agent's hook, or its statusLine command, reported.
    ///
    /// Not answered: the hook command runs inside the agent's own turn and
    /// has to be gone again in milliseconds, so it writes this and exits
    /// without waiting.
    Hook {
        /// The event otag registered the command under, which is more
        /// trustworthy than the name inside the payload.
        event: String,
        /// The pane the agent is running in, from the environment otag puts
        /// its panes in. Missing when the agent was not started by otag.
        #[serde(default)]
        pane: Option<PaneId>,
        /// Exactly what the agent wrote on the command's standard input,
        /// carried as text so that this request stays comparable and the
        /// daemon decides how to read it.
        payload: String,
    },
    /// Watch one pane as a stream of bytes, for a client that does its own
    /// terminal emulation.
    ///
    /// Answered with [`Response::PaneScreen`], whose bytes put the pane's
    /// current screen onto an empty terminal; after that the daemon sends a
    /// [`Response::Output`] for every chunk the program writes, unchanged
    /// and in order. A client that falls too far behind is sent a new
    /// [`Response::PaneScreen`] and carries on from there.
    ///
    /// `size` is how large the client is drawing the pane. Giving one means
    /// the client owns that pane's size from then on: otag's own layout
    /// stops resizing it, because only the client knows how much room it
    /// has.
    AttachPane {
        /// The pane to watch.
        pane: PaneId,
        /// How large the client is drawing it.
        #[serde(default)]
        size: Option<PaneSize>,
    },
    /// Say whether the client is drawing its panes on dark or light paper,
    /// so that a program started afterwards is told. Answered with
    /// [`Response::Ok`].
    ///
    /// otag puts `COLORFGBG` into every pane it starts from then on. It
    /// cannot change the environment of a program that is already running,
    /// so this reaches new panes only — a running agent learns the same
    /// thing by asking the pane with `OSC 11`, which it is answered
    /// truthfully and at once.
    TerminalAppearance {
        /// Whether the client's terminal theme is a dark one.
        dark: bool,
    },
    /// Stop watching one pane. Answered with [`Response::Ok`]; the pane and
    /// its program keep running.
    DetachPane {
        /// The pane to stop watching.
        pane: PaneId,
    },
    /// Write these bytes to one pane's input, named rather than focused:
    /// a client with windows of its own has no single focused pane.
    ///
    /// Not answered, for the same reason as [`Request::Input`].
    PaneInput {
        /// The pane to type into.
        pane: PaneId,
        /// Exactly what the program should receive.
        bytes: Bytes,
    },
    /// Tell one pane how large the client is drawing it.
    ///
    /// Not answered. As with [`Request::AttachPane`], this hands that pane's
    /// size to the client.
    PaneResize {
        /// The pane that changed size.
        pane: PaneId,
        /// Its new size.
        size: PaneSize,
    },
    /// Start a session of its own: a project, an agent, and what to ask it
    /// first. Answered with [`Response::SessionStarted`].
    ///
    /// This is what a client's "new session" sends. Everything but the
    /// directory is optional: no agent means the user's shell, no model
    /// means the agent's own default, and no prompt means the agent starts
    /// waiting for one.
    StartSession {
        /// The project directory the session works in.
        #[serde(default)]
        cwd: Option<PathBuf>,
        /// The agent to run, or the user's shell when there is none.
        #[serde(default)]
        agent: Option<AgentKind>,
        /// A model to ask the agent for, when the user named one.
        #[serde(default)]
        model: Option<String>,
        /// What to ask the agent first.
        #[serde(default)]
        prompt: Option<String>,
        /// Whether to give the session a git worktree of its own, so that it
        /// cannot collide with the other sessions in the same checkout.
        #[serde(default)]
        worktree: bool,
    },
    /// Be told about everything that happens in the session, without asking
    /// again: statuses, usage, questions and handoffs.
    ///
    /// Answered with [`Response::Ok`], after which the daemon pushes a
    /// [`Response::Event`] whenever something happens, until the connection
    /// ends.
    Subscribe,
    /// Have otag work through a queue of prompts for one session overnight,
    /// pausing at a share of the limits and carrying on when the window
    /// resets. Answered with [`Response::Ok`] or an error.
    ///
    /// No plan at all turns the night shift off for that session; what it
    /// has already done is kept, so the morning report still has it.
    NightShift {
        /// The session to work for.
        pane: PaneId,
        /// What to do, or nothing to stop.
        #[serde(default)]
        plan: Option<NightShiftPlan>,
    },
    /// Ask what otag did while nobody was watching. Answered with
    /// [`Response::MorningReport`].
    MorningReport,
    /// Ask for the readings behind the usage numbers, for a chart.
    /// Answered with [`Response::UsageHistory`].
    ///
    /// The daemon keeps these across restarts, so a client can draw a
    /// timeline the moment it connects rather than after watching for an
    /// hour.
    UsageHistory,
    /// List what the agents are waiting for the user to answer. Answered
    /// with [`Response::Inbox`].
    Inbox,
    /// Answer one of those questions. Answered with [`Response::Answered`].
    Answer {
        /// The question, as [`AskView::id`] gave it.
        ask: u64,
        /// What to answer.
        answer: Answer,
    },
    /// Stop watching the session and close the connection. Answered with
    /// [`Response::Ok`]. The panes and their programs keep running.
    Detach,
    /// A request this build does not know about.
    #[serde(other)]
    Unknown,
}

/// What a handoff should do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum Handoff {
    /// Open another agent on the same task, with a package to read.
    Agent {
        /// Which agent, or the other one otag knows about.
        #[serde(default)]
        to: Option<AgentKind>,
    },
    /// Continue the same session on another model of the same agent.
    Model {
        /// Which model, or the agent's fallback when otag has a verified
        /// one.
        #[serde(default)]
        model: Option<String>,
    },
    /// A mode this build does not know about.
    #[serde(other)]
    Unknown,
}

/// The daemon's answer to a [`Request`].
///
/// Not `Eq`: a usage reading carries a percentage, and a percentage is a
/// floating-point number.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    /// Answer to [`Request::Ping`].
    Pong,
    /// The request was carried out and has no payload.
    Ok,
    /// Answer to [`Request::Version`].
    Version {
        /// Version of the running daemon.
        version: String,
    },
    /// Answer to [`Request::ListPanes`].
    Panes {
        /// Every pane the daemon is running.
        panes: Vec<PaneInfo>,
    },
    /// Answer to [`Request::Usage`].
    Usage {
        /// How much of each agent's windows is used.
        report: Box<Report>,
        /// What each pane has spent, so the user can see which agent is
        /// doing the spending.
        #[serde(default)]
        panes: Vec<PaneUsage>,
    },
    /// Everything the client has to draw.
    ///
    /// Sent as the answer to [`Request::Attach`] and again whenever the
    /// layout changes, carrying the whole screen of every visible pane.
    View {
        /// What the session looks like now.
        view: Box<SessionView>,
    },
    /// A visible pane's screen changed.
    Frame {
        /// Pane the frame belongs to.
        pane: PaneId,
        /// What changed, relative to the last frame this client was sent.
        frame: ScreenFrame,
    },
    /// The session has no panes left, so there is nothing more to draw.
    ///
    /// Carries how the last pane's program ended, when that is known, so a
    /// client can report a failure whose output has scrolled away.
    SessionEnded {
        /// How the last program ended.
        #[serde(default)]
        exit: Option<PaneExit>,
    },
    /// Everything a pane has on its screen, as terminal output.
    ///
    /// The answer to [`Request::AttachPane`], and sent again whenever the
    /// daemon cannot give a client the bytes in between — a client that fell
    /// behind, or a pane that was resized. Writing these bytes into an empty
    /// terminal of the pane's size reproduces the screen; a client replaces
    /// whatever it had rather than appending.
    PaneScreen {
        /// The pane this is.
        pane: PaneId,
        /// Its size, which is the size the bytes assume.
        size: PaneSize,
        /// The escape sequences that recreate the screen.
        bytes: Bytes,
    },
    /// A chunk of a pane's output, exactly as its program wrote it.
    Output {
        /// The pane that wrote it.
        pane: PaneId,
        /// What it wrote.
        bytes: Bytes,
    },
    /// Answer to [`Request::StartSession`].
    SessionStarted {
        /// The pane the session is running in, to attach to.
        pane: PaneId,
        /// The name the sidebar gives its project.
        #[serde(default)]
        workspace: String,
        /// Where it is working, which is the worktree when one was made.
        #[serde(default)]
        cwd: Option<PathBuf>,
    },
    /// A pane a client was watching in raw mode has ended, and nothing more
    /// will come from it.
    PaneEnded {
        /// The pane that ended.
        pane: PaneId,
        /// How its program ended, when that is known.
        #[serde(default)]
        exit: Option<PaneExit>,
    },
    /// Something happened that a subscribed client wants to know about.
    Event {
        /// What happened.
        event: Event,
    },
    /// Answer to [`Request::MorningReport`].
    MorningReport {
        /// One entry per session otag worked for, newest activity last.
        sessions: Vec<SessionReport>,
    },
    /// Answer to [`Request::UsageHistory`].
    UsageHistory {
        /// Every reading otag has kept, oldest first.
        samples: Vec<otag_features::usage::Sample>,
    },
    /// Answer to [`Request::Inbox`].
    Inbox {
        /// What the agents are waiting for, oldest first.
        asks: Vec<AskView>,
    },
    /// Answer to [`Request::Answer`].
    Answered {
        /// The question that was answered.
        ask: u64,
        /// Whether the agent took the answer: otag types it into the pane
        /// and then checks that the prompt is gone.
        delivered: bool,
        /// One line saying what happened, for the user.
        #[serde(default)]
        summary: String,
    },
    /// Answer to [`Request::Handoff`].
    HandoffDone {
        /// The pane the work carries on in.
        pane: PaneId,
        /// The package that was written, when the handoff wrote one.
        #[serde(default)]
        package: Option<PathBuf>,
        /// One line saying what happened, for the user.
        #[serde(default)]
        summary: String,
    },
    /// The request could not be carried out.
    Error {
        /// Human-readable description of what went wrong.
        message: String,
    },
    /// A response this build does not know about.
    #[serde(other)]
    Unknown,
}

/// What a client asks the night shift to do for one session.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NightShiftPlan {
    /// Percentage of a window at which to stop sending, or otag's default.
    #[serde(default)]
    pub pause_at: Option<u16>,
    /// What to send when a window has reset and the queue is empty.
    #[serde(default)]
    pub resume_prompt: Option<String>,
    /// Prompts to work through, in order.
    #[serde(default)]
    pub queue: Vec<String>,
}

/// What the night shift is doing for one session, as a client sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NightShiftView {
    /// The threshold in use.
    pub pause_at: u16,
    /// Whether otag is waiting for a window to reset.
    #[serde(default)]
    pub paused: bool,
    /// How many prompts have been sent.
    #[serde(default)]
    pub sent: usize,
    /// How many are queued in total.
    #[serde(default)]
    pub queued: usize,
    /// What it sends when a window resets, if anything.
    #[serde(default)]
    pub resume_prompt: String,
}

/// What otag did for one session while nobody was watching.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionReport {
    /// Which session this is.
    pub pane: PaneId,
    /// The project it is working in.
    #[serde(default)]
    pub workspace: String,
    /// What is running in it.
    #[serde(default)]
    pub title: String,
    /// The agent otag noticed in it.
    #[serde(default)]
    pub agent: Option<AgentKind>,
    /// What that agent has spent.
    #[serde(default)]
    pub tokens: Tokens,
    /// How many prompts otag sent it.
    #[serde(default)]
    pub sent: usize,
    /// How many are still queued.
    #[serde(default)]
    pub remaining: usize,
    /// What otag did, oldest first.
    #[serde(default)]
    pub log: Vec<ReportEntry>,
    /// Commits made in its directory since the night shift started.
    #[serde(default)]
    pub commits: Vec<String>,
}

/// One line of what otag did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportEntry {
    /// When, in seconds since the epoch.
    pub at: i64,
    /// What happened.
    pub what: String,
}

/// Something the daemon tells a subscribed client about.
///
/// Every event is a complete statement rather than a difference, so a client
/// that misses one — or arrives late — is never left with a wrong picture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// The panes, or something about them, changed: a pane opened or
    /// closed, an agent was noticed, a status moved.
    Panes {
        /// Every pane the daemon is running.
        panes: Vec<PaneInfo>,
    },
    /// New numbers for how much of the agents' limits is used.
    Usage {
        /// How much of each agent's windows is used.
        report: Box<Report>,
    },
    /// An agent is waiting for an answer.
    Asked {
        /// What it is asking.
        ask: AskView,
    },
    /// A question is no longer waiting, because it was answered — from the
    /// inbox or in the pane itself — or because the pane is gone.
    Resolved {
        /// The question that is gone.
        ask: u64,
        /// What it was answered with, when otag answered it.
        #[serde(default)]
        answer: Option<Answer>,
    },
    /// A handoff finished.
    Handoff {
        /// The pane the work carries on in.
        pane: PaneId,
        /// The package that was written, when the handoff wrote one.
        #[serde(default)]
        package: Option<PathBuf>,
        /// One line saying what happened.
        #[serde(default)]
        summary: String,
    },
    /// Something worth telling the user, in one line.
    Notice {
        /// The pane it is about, when it is about one.
        #[serde(default)]
        pane: Option<PaneId>,
        /// What to say.
        text: String,
    },
    /// An event this build does not know about.
    #[serde(other)]
    Unknown,
}

/// One question waiting for the user, as a client sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskView {
    /// Which question this is, for [`Request::Answer`].
    pub id: u64,
    /// The pane whose agent is asking.
    pub pane: PaneId,
    /// The agent asking, when otag knows which one it is.
    #[serde(default)]
    pub agent: Option<AgentKind>,
    /// The tool it wants to use, in its own words.
    #[serde(default)]
    pub tool: Option<String>,
    /// One line saying what is waiting.
    #[serde(default)]
    pub summary: String,
    /// When it started waiting, in seconds since the epoch.
    #[serde(default)]
    pub asked_at: Option<u64>,
}

impl AskView {
    /// The client's form of one question.
    #[must_use]
    pub fn of(ask: &Ask) -> Self {
        Self {
            id: ask.id.get(),
            pane: PaneId::from_number(ask.pane),
            agent: ask.agent,
            tool: ask.tool.clone(),
            summary: ask.summary.clone(),
            asked_at: ask
                .asked_at
                .duration_since(SystemTime::UNIX_EPOCH)
                .ok()
                .map(|since| since.as_secs()),
        }
    }

    /// The question's identifier, as the inbox knows it.
    #[must_use]
    pub const fn ask_id(&self) -> AskId {
        AskId::from_number(self.id)
    }
}

/// Bytes on their way across the socket.
///
/// JSON has no bytes of its own. A sequence of numbers would work and is
/// what the protocol used first, so it is still accepted, but a pane's
/// output is sent often and in quantity: base64 is a third of the size and
/// far cheaper to read. Everything otag sends is base64.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Bytes(pub Vec<u8>);

impl Bytes {
    /// The bytes themselves.
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }

    /// Take the bytes out.
    #[must_use]
    pub fn into_vec(self) -> Vec<u8> {
        self.0
    }

    /// Whether there are no bytes at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<Vec<u8>> for Bytes {
    fn from(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }
}

impl From<&[u8]> for Bytes {
    fn from(bytes: &[u8]) -> Self {
        Self(bytes.to_vec())
    }
}

impl Serialize for Bytes {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        use base64::Engine as _;
        serializer.serialize_str(&base64::engine::general_purpose::STANDARD.encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for Bytes {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        /// Either form: base64 text, or the sequence of numbers the protocol
        /// used before.
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Either {
            Text(String),
            Numbers(Vec<u8>),
        }

        use base64::Engine as _;
        match Either::deserialize(deserializer)? {
            Either::Text(text) => base64::engine::general_purpose::STANDARD
                .decode(text.as_bytes())
                .map(Self)
                .map_err(serde::de::Error::custom),
            Either::Numbers(numbers) => Ok(Self(numbers)),
        }
    }
}

/// What the daemon reports about one pane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneInfo {
    /// Which pane this is.
    pub id: PaneId,
    /// Its current size.
    pub size: PaneSize,
    /// Name of the workspace it belongs to.
    #[serde(default)]
    pub workspace: String,
    /// The directory it is working in, when otag knows.
    ///
    /// What a client needs to offer "another session in this project": the
    /// workspace's name is a folder name, which is not a path.
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    /// What it is running.
    #[serde(default)]
    pub title: String,
    /// The agent otag has noticed in it.
    #[serde(default)]
    pub agent: Option<AgentKind>,
    /// What that agent is doing.
    #[serde(default)]
    pub status: PaneStatus,
    /// The agent session otag would resume.
    #[serde(default)]
    pub session: Option<String>,
    /// The first thing the user asked this session, when otag knows it.
    ///
    /// What a client calls the session. Every session in one folder is
    /// otherwise named after that folder, which is the same as having no
    /// names at all. It comes from the prompt a client started the session
    /// with, or from the agent's own transcript for a session otag did not
    /// start, and it is clipped so that a pasted essay cannot travel as a
    /// name.
    #[serde(default)]
    pub first_prompt: Option<String>,
    /// What the agent in it has spent.
    #[serde(default)]
    pub tokens: Tokens,
    /// How large the model's context is, when otag knows. Without it a
    /// context meter has no denominator and is not drawn as a share.
    #[serde(default)]
    pub context_window: Option<u64>,
    /// What the night shift is doing for it, when it has been asked to.
    #[serde(default)]
    pub night_shift: Option<NightShiftView>,
    /// How its program ended, if it has.
    #[serde(default)]
    pub exit: Option<PaneExit>,
}

/// What one pane has spent, for attributing usage to the agent that caused
/// it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneUsage {
    /// Which pane this is.
    pub pane: PaneId,
    /// The agent otag has noticed in it.
    #[serde(default)]
    pub agent: Option<AgentKind>,
    /// What that agent has spent.
    #[serde(default)]
    pub tokens: Tokens,
}

/// One workspace, as the sidebar shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceView {
    /// Which workspace this is.
    pub id: WorkspaceId,
    /// What the sidebar calls it.
    pub name: String,
    /// How many tabs it has.
    pub tabs: usize,
    /// How many panes it has, across all its tabs.
    pub panes: usize,
    /// What its agents are doing, taken from the pane that most wants the
    /// user.
    #[serde(default)]
    pub status: PaneStatus,
}

/// One tab of the active workspace, as the tab bar shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabView {
    /// Which tab this is.
    pub id: TabId,
    /// What the tab bar calls it.
    pub title: String,
    /// How many panes it holds.
    pub panes: usize,
    /// What its agents are doing, taken from the pane that most wants the
    /// user.
    #[serde(default)]
    pub status: PaneStatus,
}

/// One visible pane: where it is, and what is on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneView {
    /// Which pane this is.
    pub id: PaneId,
    /// The rectangle it occupies, border included.
    pub area: Area,
    /// What its border says it is running.
    #[serde(default)]
    pub title: String,
    /// The agent otag has noticed in it.
    #[serde(default)]
    pub agent: Option<AgentKind>,
    /// What that agent is doing.
    #[serde(default)]
    pub status: PaneStatus,
    /// Its whole screen, when the daemon is sending a full view.
    #[serde(default)]
    pub frame: Option<ScreenFrame>,
    /// How its program ended, if it has.
    #[serde(default)]
    pub exit: Option<PaneExit>,
}

/// Everything a client draws: the furniture, the layout, and the panes.
///
/// The daemon computes the geometry, including where its own sidebar and tab
/// bar go, because it is also what tells each pane's program how large it is.
/// Two clients of one session therefore cannot disagree about where a pane
/// is.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionView {
    /// Counter of structural changes, for logs and tests.
    #[serde(default)]
    pub generation: u64,
    /// Size of the terminal this view was computed for.
    pub size: PaneSize,
    /// Where the workspace sidebar goes; empty when it is hidden.
    #[serde(default)]
    pub sidebar: Area,
    /// Where the tab bar goes.
    #[serde(default)]
    pub tabs_area: Area,
    /// The space the panes divide between them.
    #[serde(default)]
    pub content: Area,
    /// Every workspace, in the order the sidebar lists them.
    #[serde(default)]
    pub workspaces: Vec<WorkspaceView>,
    /// Which workspace is shown, as an index into `workspaces`.
    #[serde(default)]
    pub active_workspace: usize,
    /// The active workspace's tabs, left to right.
    #[serde(default)]
    pub tabs: Vec<TabView>,
    /// Which tab is shown, as an index into `tabs`.
    #[serde(default)]
    pub active_tab: usize,
    /// The panes of the active tab.
    #[serde(default)]
    pub panes: Vec<PaneView>,
    /// The pane the user is typing into.
    #[serde(default)]
    pub focus: Option<PaneId>,
    /// How much of the agents' limits is used, for the widget.
    #[serde(default)]
    pub usage: Report,
}

impl SessionView {
    /// The visible pane at this cell, if there is one.
    ///
    /// Used to turn a click into a focus change; a click on a pane's border
    /// counts as a click on the pane, because the border is part of what the
    /// user sees as that pane.
    #[must_use]
    pub fn pane_at(&self, x: u16, y: u16) -> Option<PaneId> {
        self.panes
            .iter()
            .find(|pane| pane.area.contains(x, y))
            .map(|pane| pane.id)
    }

    /// Where one visible pane is.
    #[must_use]
    pub fn pane(&self, id: PaneId) -> Option<&PaneView> {
        self.panes.iter().find(|pane| pane.id == id)
    }

    /// The pane the user is typing into.
    #[must_use]
    pub fn focused(&self) -> Option<&PaneView> {
        self.focus.and_then(|focus| self.pane(focus))
    }

    /// Whether the session has nothing left to draw.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.panes.is_empty()
    }
}

impl Response {
    /// Build an [`Response::Error`] from anything string-like.
    pub fn error(message: impl Into<String>) -> Self {
        Self::Error {
            message: message.into(),
        }
    }

    /// Build a [`Response::View`] from a view.
    #[must_use]
    pub fn view(view: SessionView) -> Self {
        Self::View {
            view: Box::new(view),
        }
    }
}

/// Encode a value as a single protocol line, including the trailing newline.
pub fn encode_line<T: Serialize>(value: &T) -> Result<String, serde_json::Error> {
    let mut line = serde_json::to_string(value)?;
    line.push('\n');
    Ok(line)
}

/// Decode a single protocol line. The trailing newline is optional.
pub fn decode_line<T: for<'de> Deserialize<'de>>(line: &str) -> Result<T, serde_json::Error> {
    serde_json::from_str(line.trim_end_matches(['\n', '\r']))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{CellStyle, CursorShape, CursorState, InputModes, ScreenRow, StyledRun};

    fn roundtrip_request(request: Request) {
        let line = encode_line(&request).expect("request encodes");
        assert!(line.ends_with('\n'));
        let decoded: Request = decode_line(&line).expect("request decodes");
        assert_eq!(decoded, request);
    }

    fn frame() -> ScreenFrame {
        ScreenFrame {
            size: PaneSize::new(2, 8),
            cursor: CursorState {
                row: 1,
                col: 3,
                visible: true,
                shape: CursorShape::SteadyBlock,
            },
            modes: InputModes {
                application_cursor: true,
                ..InputModes::default()
            },
            full: true,
            rows: vec![ScreenRow {
                index: 0,
                runs: vec![StyledRun {
                    text: "hi".to_owned(),
                    style: CellStyle::default(),
                }],
            }],
        }
    }

    fn view() -> SessionView {
        SessionView {
            generation: 7,
            size: PaneSize::new(24, 80),
            sidebar: Area::new(0, 0, 16, 24),
            tabs_area: Area::new(16, 0, 64, 1),
            content: Area::new(16, 1, 64, 23),
            workspaces: vec![WorkspaceView {
                id: WorkspaceId::FIRST,
                name: "app".to_owned(),
                tabs: 1,
                panes: 1,
                status: PaneStatus::Working,
            }],
            active_workspace: 0,
            tabs: vec![TabView {
                id: TabId::FIRST,
                title: "shell".to_owned(),
                panes: 1,
                status: PaneStatus::Working,
            }],
            active_tab: 0,
            panes: vec![PaneView {
                id: PaneId::FIRST,
                area: Area::new(16, 1, 64, 23),
                title: "claude".to_owned(),
                agent: Some(AgentKind::Claude),
                status: PaneStatus::Working,
                frame: Some(frame()),
                exit: None,
            }],
            focus: Some(PaneId::FIRST),
            usage: Default::default(),
        }
    }

    #[test]
    fn requests_roundtrip() {
        roundtrip_request(Request::Ping);
        roundtrip_request(Request::Version);
        roundtrip_request(Request::Shutdown);
        roundtrip_request(Request::ListPanes);
        roundtrip_request(Request::Detach);
        roundtrip_request(Request::Attach {
            size: PaneSize::new(30, 100),
            cwd: None,
        });
        roundtrip_request(Request::Attach {
            size: PaneSize::DEFAULT,
            cwd: Some(PathBuf::from("/some/project")),
        });
        roundtrip_request(Request::Input {
            bytes: Bytes(vec![0x1b, b'[', b'A']),
        });
        roundtrip_request(Request::Resize {
            size: PaneSize::new(24, 80),
        });
        roundtrip_request(Request::Split {
            direction: Direction::LeftRight,
        });
        roundtrip_request(Request::Split {
            direction: Direction::TopBottom,
        });
        roundtrip_request(Request::NewTab);
        roundtrip_request(Request::SelectTab { index: 3 });
        roundtrip_request(Request::NewWorkspace { cwd: None });
        roundtrip_request(Request::NewWorkspace {
            cwd: Some(PathBuf::from("/other/project")),
        });
        roundtrip_request(Request::SelectWorkspace { index: 1 });
        roundtrip_request(Request::FocusPane {
            pane: PaneId::FIRST,
        });
        roundtrip_request(Request::FocusSide { side: Side::Left });
        roundtrip_request(Request::FocusNext);
        roundtrip_request(Request::ResizePane {
            side: Side::Down,
            cells: 2,
        });
        roundtrip_request(Request::ToggleSidebar);
    }

    #[test]
    fn starting_a_session_roundtrips_with_and_without_its_options() {
        roundtrip_request(Request::StartSession {
            cwd: Some(PathBuf::from("/some/project")),
            agent: Some(AgentKind::Claude),
            model: Some("sonnet".to_owned()),
            prompt: Some("read the roadmap".to_owned()),
            worktree: true,
        });
        roundtrip_request(Request::StartSession {
            cwd: None,
            agent: None,
            model: None,
            prompt: None,
            worktree: false,
        });

        // A client that sends only a directory gets a shell in it.
        let decoded: Request =
            decode_line(r#"{"type":"start_session","cwd":"/some/project"}"#).expect("decodes");
        assert_eq!(
            decoded,
            Request::StartSession {
                cwd: Some(PathBuf::from("/some/project")),
                agent: None,
                model: None,
                prompt: None,
                worktree: false,
            }
        );
    }

    #[test]
    fn the_requests_a_client_with_its_own_terminal_sends_roundtrip() {
        roundtrip_request(Request::AttachPane {
            pane: PaneId::FIRST,
            size: Some(PaneSize::new(30, 100)),
        });
        roundtrip_request(Request::AttachPane {
            pane: PaneId::FIRST,
            size: None,
        });
        roundtrip_request(Request::DetachPane {
            pane: PaneId::FIRST,
        });
        roundtrip_request(Request::PaneInput {
            pane: PaneId::FIRST,
            bytes: Bytes(b"ls\r".to_vec()),
        });
        roundtrip_request(Request::PaneResize {
            pane: PaneId::FIRST,
            size: PaneSize::new(24, 80),
        });
        roundtrip_request(Request::Subscribe);
        roundtrip_request(Request::Inbox);
        roundtrip_request(Request::Answer {
            ask: 7,
            answer: Answer::Allow,
        });
        roundtrip_request(Request::Answer {
            ask: 8,
            answer: Answer::Deny,
        });
    }

    #[test]
    fn bytes_travel_as_base64_and_are_also_read_as_numbers() {
        // base64 is a third of the size of a list of numbers, and a pane's
        // output is the one message otag sends in quantity.
        let encoded = serde_json::to_string(&Bytes(vec![0x1b, b'[', b'A'])).expect("encodes");
        assert_eq!(encoded, r#""G1tB""#);

        let from_text: Bytes = serde_json::from_str(r#""G1tB""#).expect("decodes base64");
        assert_eq!(from_text.as_slice(), [0x1b, b'[', b'A']);

        // The protocol used a list of numbers first, so one still decodes.
        let from_numbers: Bytes = serde_json::from_str("[27,91,65]").expect("decodes numbers");
        assert_eq!(from_numbers, from_text);
    }

    #[test]
    fn bytes_that_are_not_base64_are_refused_rather_than_guessed_at() {
        assert!(serde_json::from_str::<Bytes>(r#""not base64!""#).is_err());
    }

    #[test]
    fn an_answer_otag_does_not_understand_is_refused() {
        // Typing a guess into a running agent is worse than an error.
        assert!(
            decode_line::<Request>(r#"{"type":"answer","ask":1,"answer":"maybe"}"#).is_err(),
            "an unknown answer has no catch-all"
        );
    }

    #[test]
    fn an_unknown_event_decodes_rather_than_failing() {
        let decoded: Response =
            decode_line(r#"{"type":"event","event":{"event":"invented_later"}}"#).expect("decodes");
        assert_eq!(
            decoded,
            Response::Event {
                event: Event::Unknown
            }
        );
    }

    #[test]
    fn attaching_without_a_directory_is_allowed() {
        let decoded: Request =
            decode_line(r#"{"type":"attach","size":{"rows":10,"cols":20}}"#).expect("decodes");
        assert_eq!(
            decoded,
            Request::Attach {
                size: PaneSize::new(10, 20),
                cwd: None,
            }
        );
    }

    #[test]
    fn an_attach_with_an_impossible_size_is_clamped_not_rejected() {
        let decoded: Request =
            decode_line(r#"{"type":"attach","size":{"rows":0,"cols":0}}"#).expect("decodes");
        assert_eq!(
            decoded,
            Request::Attach {
                size: PaneSize::new(1, 1),
                cwd: None,
            }
        );
    }

    #[test]
    fn responses_roundtrip() {
        for response in [
            Response::Pong,
            Response::Ok,
            Response::Version {
                version: "0.1.0".to_owned(),
            },
            Response::Panes {
                panes: vec![PaneInfo {
                    id: PaneId::FIRST,
                    size: PaneSize::DEFAULT,
                    workspace: "app".to_owned(),
                    cwd: Some(PathBuf::from("/projects/app")),
                    title: "shell".to_owned(),
                    agent: None,
                    status: PaneStatus::Idle,
                    session: None,
                    first_prompt: None,
                    tokens: Default::default(),
                    context_window: None,
                    night_shift: None,
                    exit: None,
                }],
            },
            Response::Panes {
                panes: vec![PaneInfo {
                    id: PaneId::FIRST,
                    size: PaneSize::DEFAULT,
                    workspace: "app".to_owned(),
                    cwd: Some(PathBuf::from("/projects/app")),
                    title: "shell".to_owned(),
                    agent: Some(AgentKind::Claude),
                    status: PaneStatus::Working,
                    session: Some("a-session".to_owned()),
                    first_prompt: Some("add a parser test".to_owned()),
                    tokens: Default::default(),
                    context_window: Some(200_000),
                    night_shift: Some(NightShiftView {
                        pause_at: 90,
                        paused: false,
                        sent: 1,
                        queued: 3,
                        resume_prompt: "carry on".to_owned(),
                    }),
                    exit: Some(PaneExit {
                        code: Some(0),
                        signal: None,
                    }),
                }],
            },
            Response::view(view()),
            Response::Frame {
                pane: PaneId::FIRST,
                frame: frame(),
            },
            Response::SessionEnded { exit: None },
            Response::SessionEnded {
                exit: Some(PaneExit {
                    code: None,
                    signal: Some("SIGTERM".to_owned()),
                }),
            },
            Response::error("boom"),
        ] {
            let line = encode_line(&response).expect("response encodes");
            let decoded: Response = decode_line(&line).expect("response decodes");
            assert_eq!(decoded, response);
        }
    }

    #[test]
    fn request_tags_are_snake_case() {
        let line = encode_line(&Request::Ping).expect("encodes");
        assert_eq!(line, "{\"type\":\"ping\"}\n");
        let line = encode_line(&Request::NewTab).expect("encodes");
        assert_eq!(line, "{\"type\":\"new_tab\"}\n");
    }

    #[test]
    fn a_pane_listing_tolerates_a_missing_exit() {
        let decoded: Response =
            decode_line(r#"{"type":"panes","panes":[{"id":1,"size":{"rows":5,"cols":5}}]}"#)
                .expect("decodes");
        let Response::Panes { panes } = decoded else {
            panic!("expected a pane listing");
        };
        assert_eq!(panes[0].exit, None);
        assert_eq!(panes[0].title, "");
    }

    #[test]
    fn a_view_tolerates_everything_optional_being_absent() {
        // A daemon that has nothing to show yet, or an older one, must not
        // put the client into a broken state.
        let decoded: Response =
            decode_line(r#"{"type":"view","view":{"size":{"rows":5,"cols":9}}}"#).expect("decodes");
        let Response::View { view } = decoded else {
            panic!("expected a view");
        };
        assert_eq!(view.size, PaneSize::new(5, 9));
        assert!(view.is_empty());
        assert_eq!(view.focus, None);
        assert_eq!(view.generation, 0);
    }

    #[test]
    fn an_unknown_direction_or_side_decodes_rather_than_failing() {
        let decoded: Request =
            decode_line(r#"{"type":"split","direction":"sideways"}"#).expect("decodes");
        assert_eq!(
            decoded,
            Request::Split {
                direction: Direction::Unknown
            }
        );
        let decoded: Request =
            decode_line(r#"{"type":"focus_side","side":"inwards"}"#).expect("decodes");
        assert_eq!(
            decoded,
            Request::FocusSide {
                side: Side::Unknown
            }
        );
    }

    #[test]
    fn a_view_finds_the_pane_under_a_click() {
        let view = view();
        assert_eq!(view.pane_at(20, 5), Some(PaneId::FIRST));
        assert_eq!(view.pane_at(0, 0), None, "the sidebar is not a pane");
        assert_eq!(view.focused().map(|pane| pane.id), Some(PaneId::FIRST));
    }

    #[test]
    fn unknown_request_type_decodes_to_unknown() {
        let decoded: Request = decode_line("{\"type\":\"teleport\"}").expect("decodes");
        assert_eq!(decoded, Request::Unknown);
    }

    #[test]
    fn unknown_response_type_decodes_to_unknown() {
        let decoded: Response = decode_line("{\"type\":\"teleported\"}").expect("decodes");
        assert_eq!(decoded, Response::Unknown);
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let decoded: Request =
            decode_line("{\"type\":\"ping\",\"from_the_future\":42}").expect("decodes");
        assert_eq!(decoded, Request::Ping);
    }

    #[test]
    fn carriage_return_is_tolerated() {
        let decoded: Request = decode_line("{\"type\":\"ping\"}\r\n").expect("decodes");
        assert_eq!(decoded, Request::Ping);
    }

    #[test]
    fn truncated_line_is_an_error_not_a_panic() {
        assert!(decode_line::<Request>("{\"type\":\"pi").is_err());
        assert!(decode_line::<Request>("").is_err());
        assert!(decode_line::<Request>("not json at all").is_err());
    }
}
