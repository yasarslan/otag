//! Everything otag knows about the agents it runs.
//!
//! Each agent keeps its sessions in its own place, in its own format, and
//! shows its own words on screen. All of that lives behind [`AgentAdapter`],
//! and nowhere else in otag: these formats change with every release of the
//! agents, and a change has to be a change to one file with one set of
//! fixtures behind it.
//!
//! Nothing here is allowed to assume a field is present. A transcript line
//! from a newer agent, a hook payload from an older one, and a statusLine
//! that an API-key account produces without any usage in it must all parse
//! into whatever part of them is understood.
#![forbid(unsafe_code)]

pub mod approval;
pub mod claude;
pub mod codex;
pub mod hooks;
pub mod provider;
pub mod settings;
pub mod team;

use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

/// An agent otag knows how to work with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    /// Claude Code.
    Claude,
    /// Codex CLI.
    Codex,
    /// API-backed compatible agent with session-local credentials.
    Deepseek,
}

impl AgentKind {
    /// Every agent otag has an adapter for.
    pub const ALL: [Self; 3] = [Self::Claude, Self::Codex, Self::Deepseek];

    /// The name otag shows and accepts on the command line.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Deepseek => "deepseek",
        }
    }

    /// The name otag shows in the interface, where there is room for a
    /// capital letter.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
            Self::Deepseek => "DeepSeek",
        }
    }

    /// Whether otag shows this agent's limit windows before it has reported
    /// one.
    ///
    /// Claude Code reports the account's limits with every reply, so its two
    /// windows are always worth a line: a line with a dash in it says "no
    /// reading yet", which is a different thing from the window not
    /// existing. Codex reports limits only in some of its session files, so
    /// a window of its own appears once it has reported one.
    #[must_use]
    pub const fn shows_limit_windows(self) -> bool {
        matches!(self, Self::Claude)
    }

    /// The adapter for this agent.
    #[must_use]
    pub fn adapter(self) -> &'static dyn AgentAdapter {
        match self {
            Self::Claude => &claude::Claude,
            Self::Codex => &codex::Codex,
            Self::Deepseek => &provider::Deepseek,
        }
    }
}

impl fmt::Display for AgentKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

impl FromStr for AgentKind {
    type Err = Error;

    fn from_str(name: &str) -> Result<Self> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.name().eq_ignore_ascii_case(name))
            .ok_or_else(|| Error::UnknownAgent {
                name: name.to_owned(),
            })
    }
}

/// What an agent in a pane is doing.
///
/// Ordered by how much it wants the user: a sidebar showing one status for a
/// workspace shows the largest of its panes'.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaneStatus {
    /// Waiting for the user, with nothing to say.
    #[default]
    Idle,
    /// Working on something.
    Working,
    /// Finished what it was asked.
    Done,
    /// Repeating itself: the same error or the same edit, over and over.
    Looping,
    /// Something went wrong.
    Error,
    /// Waiting for permission.
    NeedsApproval,
}

impl PaneStatus {
    /// Whether this status is worth telling the user about.
    #[must_use]
    pub const fn wants_attention(self) -> bool {
        matches!(self, Self::NeedsApproval | Self::Error | Self::Looping)
    }

    /// The word otag shows for it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Done => "done",
            Self::Looping => "looping",
            Self::Error => "error",
            Self::NeedsApproval => "needs approval",
        }
    }
}

impl fmt::Display for PaneStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

/// One of an agent's saved sessions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRef {
    /// The identifier the agent resumes by.
    pub id: String,
    /// The file the agent keeps it in.
    pub path: PathBuf,
    /// The directory it was working in, when the file says.
    #[serde(default)]
    pub cwd: Option<PathBuf>,
}

/// How many tokens a session has used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Tokens {
    /// Tokens sent, not counting the ones read from the cache.
    pub input: u64,
    /// Tokens produced.
    pub output: u64,
    /// Tokens read from the prompt cache.
    pub cache_read: u64,
    /// Tokens written to the prompt cache.
    pub cache_written: u64,
    /// Size of the context the last turn was sent, which is what a context
    /// meter shows.
    pub context: u64,
}

impl Tokens {
    /// Everything that had to be paid for, cache reads included.
    #[must_use]
    pub const fn total(&self) -> u64 {
        self.input
            .saturating_add(self.output)
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_written)
    }
}

/// One of an agent's usage windows.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LimitWindow {
    /// How much of the window is used, as a percentage.
    pub used_pct: f32,
    /// When it resets, in seconds since the epoch.
    #[serde(default)]
    pub resets_at: Option<i64>,
}

/// What an agent says about how much of its limits are used.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Limits {
    /// The shorter window, five hours for both agents otag knows.
    #[serde(default)]
    pub five_hour: Option<LimitWindow>,
    /// The longer window, a week for both agents otag knows.
    #[serde(default)]
    pub weekly: Option<LimitWindow>,
}

impl Limits {
    /// Whether either window is known.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.five_hour.is_none() && self.weekly.is_none()
    }
}

/// What otag reads out of an agent's session file.
///
/// The same shape for every agent, so that everything built on top of it —
/// the usage widget, the handoff package, the conflict radar — is written
/// once.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Transcript {
    /// The identifier the agent would resume by.
    pub session_id: Option<String>,
    /// The directory the session was working in.
    pub cwd: Option<PathBuf>,
    /// The branch it was on, when the file says.
    pub git_branch: Option<String>,
    /// The model it was last using.
    pub model: Option<String>,
    /// What the user asked, oldest first.
    pub prompts: Vec<String>,
    /// The last thing the agent said.
    pub last_message: Option<String>,
    /// Final answer confirmed by a completed turn, excluding commentary.
    pub final_message: Option<String>,
    /// An interrupted turn must never trigger automatic progression.
    pub interrupted: bool,
    /// Tool events show an attempted file modification.
    pub edit_attempt: bool,
    /// Files it wrote to, in the order it first touched them.
    pub files_touched: Vec<PathBuf>,
    /// Shell commands it ran, oldest first.
    pub commands: Vec<String>,
    /// How many times the agent answered.
    pub turns: usize,
    /// How many tokens it used.
    pub tokens: Tokens,
    /// What it last said about its limits, when the file carries that.
    pub limits: Limits,
    /// How large the model's context is, when the file says.
    pub context_window: Option<u64>,
    /// Whether the file says a turn is running, when it says anything.
    pub turn: Option<TurnState>,
}

impl Transcript {
    /// The first thing the user actually asked, skipping what the agent put
    /// in front of it.
    ///
    /// Both agents write injected context into the transcript as a message
    /// from the user: Codex opens every rollout with an
    /// `<environment_context>` block naming the directory and the shell, and
    /// Claude Code wraps slash commands and reminders in tags of their own.
    /// None of those is anybody's prompt, and naming a session after one
    /// would be worse than not naming it.
    ///
    /// So the first prompt is the first one that reads like a person typing:
    /// it does not begin with a tag, and it is not one of the markers either
    /// agent uses. Verified against real session files from both agents on
    /// this machine before it was written.
    #[must_use]
    pub fn first_prompt(&self) -> Option<&str> {
        self.prompts
            .iter()
            .map(String::as_str)
            .map(str::trim)
            .find(|prompt| is_a_persons_prompt(prompt))
    }
}

/// Tags that mean a message came from the agent rather than from the user.
///
/// Matched anywhere in the text, not only at the start, because Claude Code
/// appends a reminder to the end of a prompt as often as it prepends one.
const INJECTED: &[&str] = &[
    "<environment_context>",
    "<user_instructions>",
    "<system-reminder>",
    "<command-name>",
    "<local-command-stdout>",
    "<command-message>",
    "<ide_opened_file>",
    "Caveat: The messages below",
];

/// Whether a message from the user is one a person typed.
#[must_use]
fn is_a_persons_prompt(prompt: &str) -> bool {
    if prompt.is_empty() {
        return false;
    }
    // Anything that opens with a tag is a block the agent put there; a
    // person writing about `<T>` does not start the line with it.
    if prompt.starts_with('<') {
        return false;
    }
    !INJECTED.iter().any(|marker| prompt.contains(marker))
}

/// What an agent's own session file says about its last turn.
///
/// An agent without hooks reports nothing to otag, but it does write its own
/// file, and that file says where a turn began and where it ended. That is a
/// far better answer than the words on its screen, which lag behind it and
/// change with every release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnState {
    /// A turn has begun and has not ended.
    Working,
    /// The last turn otag can see has ended.
    Finished,
}

impl TurnState {
    /// The pane status this turn state means.
    #[must_use]
    pub const fn status(self) -> PaneStatus {
        match self {
            Self::Working => PaneStatus::Working,
            Self::Finished => PaneStatus::Done,
        }
    }
}

/// Everything otag needs to know about one agent.
///
/// Implementations read files written by another program: every method has to
/// cope with a file in a format it does not recognise, and say so by
/// returning nothing rather than by failing.
pub trait AgentAdapter: Send + Sync {
    /// Which agent this is.
    fn kind(&self) -> AgentKind;

    /// Whether this process is that agent.
    ///
    /// `exe` is the program's name without its directory; `argv` is what it
    /// was started with, including the program itself.
    fn matches_process(&self, exe: &str, argv: &[String]) -> bool;

    /// Whether this agent's screen is asking the user for permission.
    ///
    /// Narrower than [`Self::detect_status`]: a pane can need the user for
    /// other reasons, and only a prompt may be answered with keys.
    fn is_asking(&self, screen: &str) -> bool;

    /// What to type into the agent's prompt to answer it.
    ///
    /// Only sent to a pane whose screen is asking, and only once per
    /// question; see [`crate::approval`].
    fn answer_keys(&self, answer: approval::Answer) -> Vec<u8>;

    /// One line saying what the agent is asking, read off its screen.
    ///
    /// For an agent with hooks the payload says it better; this is for the
    /// agents that have none, and for a question otag noticed without one.
    /// `None` when the screen is not asking anything otag recognises.
    fn asking_summary(&self, screen: &str) -> Option<String>;

    /// What the agent's screen says it is doing, when it says anything.
    ///
    /// This is the fallback: hooks are better where an agent has them. The
    /// words an agent puts on screen change often, so a screen that matches
    /// nothing gives `None` rather than a guess.
    fn detect_status(&self, screen: &str) -> Option<PaneStatus>;

    /// Where the agent keeps its sessions, below a home directory.
    fn session_store(&self, home: &Path) -> PathBuf;

    /// The agent's newest session for this directory.
    ///
    /// `started_after` drops sessions older than the pane asking, so that a
    /// new pane does not adopt the session of the one before it.
    fn find_session_for(
        &self,
        home: &Path,
        cwd: &Path,
        started_after: Option<SystemTime>,
    ) -> Option<SessionRef>;

    /// The command that continues a session where it left off.
    fn resume_command(&self, session: &SessionRef) -> Vec<String>;

    /// The command that starts the agent, with a first prompt and a model
    /// if there are any.
    ///
    /// A model is named only when the user asked for one: letting the agent
    /// pick its own default is the right behaviour, and otag does not know
    /// which models an account has.
    fn start_command(&self, initial_prompt: Option<&str>, model: Option<&str>) -> Vec<String>;

    /// The command that continues a session on another model.
    ///
    /// Used when one model's quota is exhausted but the agent still has
    /// some, so the conversation can carry on rather than being handed over.
    fn resume_on_model(&self, session: &SessionRef, model: &str) -> Vec<String>;

    /// Read what otag needs out of one of the agent's session files.
    fn read_transcript(&self, path: &Path) -> Result<Transcript>;
}

/// The line of a screen that a marker appears on, tidied up for a list.
///
/// Agents draw their questions inside boxes, so the line carries border
/// characters and padding that mean nothing to a reader.
#[must_use]
pub fn asking_line(screen: &str, markers: &[&str]) -> Option<String> {
    let line = screen
        .lines()
        .find(|line| markers.iter().any(|marker| line.contains(marker)))?;
    let trimmed = line
        .trim()
        .trim_matches(|character: char| {
            character.is_whitespace() || "\u{2502}\u{2503}\u{2551}|>\u{276f}*".contains(character)
        })
        .trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

/// The adapter for a process, if it is an agent otag knows.
#[must_use]
pub fn adapter_for_process(exe: &str, argv: &[String]) -> Option<&'static dyn AgentAdapter> {
    if provider::Deepseek.matches_process(exe, argv) {
        return Some(&provider::Deepseek);
    }
    AgentKind::ALL
        .into_iter()
        .map(AgentKind::adapter)
        .find(|adapter| adapter.matches_process(exe, argv))
}

/// Everything that can go wrong while reading an agent's files.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A session file could not be read.
    #[error("cannot read the {agent} session file {path}: {source}")]
    Unreadable {
        /// Which agent's file it is.
        agent: AgentKind,
        /// Path of the file.
        path: PathBuf,
        /// What the filesystem reported.
        #[source]
        source: std::io::Error,
    },

    /// A name that is not an agent otag knows.
    #[error("{name:?} is not an agent otag knows. Try one of: claude, codex")]
    UnknownAgent {
        /// The name that was given.
        name: String,
    },

    /// A settings file could not be read or written.
    #[error("{what} {path}: {source}")]
    Settings {
        /// What was being attempted, as a phrase.
        what: String,
        /// Path of the file.
        path: PathBuf,
        /// What went wrong.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

/// Convenience alias for results produced by this crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Where the fixtures used by this crate's tests live.
#[cfg(test)]
pub(crate) const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures");

#[cfg(test)]
mod tests {
    use super::*;

    /// Reading one of this repository's fixtures.
    fn fixture(name: &str) -> std::path::PathBuf {
        std::path::Path::new(FIXTURES).join(name)
    }

    #[test]
    fn the_first_prompt_is_the_first_thing_a_person_asked() {
        let transcript = AgentKind::Codex
            .adapter()
            .read_transcript(&fixture("codex-rollout.jsonl"))
            .expect("a transcript");
        assert_eq!(transcript.first_prompt(), Some("add a test for the parser"));
    }

    #[test]
    fn an_injected_context_block_is_not_the_first_prompt() {
        // Verified against a real rollout on this machine before this was
        // written: every Codex session opens with an `<environment_context>`
        // block written as a message from the user. Naming a session after
        // it would be worse than not naming it.
        let transcript = AgentKind::Codex
            .adapter()
            .read_transcript(&fixture("codex-rollout-injected-context.jsonl"))
            .expect("a transcript");

        assert!(
            transcript.prompts[0].starts_with("<environment_context>"),
            "the block is still carried, for the handoff package, and the \
             fixture still has the shape this test is about: {:?}",
            transcript.prompts.first()
        );
        assert_eq!(
            transcript.first_prompt(),
            Some("make the parser accept an empty file"),
            "the name is what the person typed"
        );
    }

    #[test]
    fn a_claude_transcript_names_itself_from_its_first_prompt() {
        let transcript = AgentKind::Claude
            .adapter()
            .read_transcript(&fixture("claude-transcript.jsonl"))
            .expect("a transcript");
        assert_eq!(
            transcript.first_prompt(),
            transcript.prompts.first().map(String::as_str)
        );
        assert!(transcript.first_prompt().is_some());
    }

    #[test]
    fn a_transcript_with_nothing_a_person_said_has_no_name() {
        assert_eq!(Transcript::default().first_prompt(), None);

        let transcript = Transcript {
            prompts: vec![
                "<environment_context><cwd>/x</cwd></environment_context>".to_owned(),
                "   ".to_owned(),
                "<system-reminder>be good</system-reminder>".to_owned(),
            ],
            ..Transcript::default()
        };
        assert_eq!(transcript.first_prompt(), None);
    }

    #[test]
    fn a_marker_anywhere_in_a_prompt_disqualifies_it() {
        // Claude Code appends a reminder to the end of a prompt as often as
        // it puts one in front.
        let transcript = Transcript {
            prompts: vec![
                "fix the build\n<system-reminder>remember this</system-reminder>".to_owned(),
                "fix the build".to_owned(),
            ],
            ..Transcript::default()
        };
        assert_eq!(transcript.first_prompt(), Some("fix the build"));
    }

    #[test]
    fn a_prompt_that_merely_mentions_a_tag_is_still_a_prompt() {
        let transcript = Transcript {
            prompts: vec!["make Vec<T> generic over the allocator".to_owned()],
            ..Transcript::default()
        };
        assert_eq!(
            transcript.first_prompt(),
            Some("make Vec<T> generic over the allocator")
        );
    }

    #[test]
    fn every_agent_has_an_adapter_that_reports_itself() {
        for kind in AgentKind::ALL {
            assert_eq!(kind.adapter().kind(), kind);
        }
    }

    #[test]
    fn an_agent_is_named_the_way_it_is_typed() {
        assert_eq!(
            "claude".parse::<AgentKind>().expect("a kind"),
            AgentKind::Claude
        );
        assert_eq!(
            "CODEX".parse::<AgentKind>().expect("a kind"),
            AgentKind::Codex
        );
        assert_eq!(AgentKind::Claude.to_string(), "claude");

        let error = "gemini".parse::<AgentKind>().expect_err("not an agent");
        assert!(error.to_string().contains("claude, codex"), "{error}");
    }

    #[test]
    fn a_status_that_wants_the_user_is_told_apart_from_one_that_does_not() {
        assert!(PaneStatus::NeedsApproval.wants_attention());
        assert!(PaneStatus::Error.wants_attention());
        assert!(PaneStatus::Looping.wants_attention());
        assert!(!PaneStatus::Working.wants_attention());
        assert!(!PaneStatus::Idle.wants_attention());
        assert!(!PaneStatus::Done.wants_attention());
    }

    #[test]
    fn the_status_that_wants_the_user_most_is_the_largest() {
        // A workspace shows one status for all its panes, and it has to be
        // the one worth acting on.
        let mut statuses = [
            PaneStatus::Idle,
            PaneStatus::NeedsApproval,
            PaneStatus::Working,
        ];
        statuses.sort_unstable();
        assert_eq!(statuses.last(), Some(&PaneStatus::NeedsApproval));
        assert_eq!(PaneStatus::default(), PaneStatus::Idle);
    }

    #[test]
    fn a_status_is_named_for_the_sidebar() {
        assert_eq!(PaneStatus::NeedsApproval.to_string(), "needs approval");
        assert_eq!(PaneStatus::Working.to_string(), "working");
    }

    #[test]
    fn tokens_add_up_to_what_was_paid_for() {
        let tokens = Tokens {
            input: 10,
            output: 20,
            cache_read: 30,
            cache_written: 40,
            context: 1000,
        };
        assert_eq!(tokens.total(), 100, "the context is not an extra cost");
    }

    #[test]
    fn tokens_that_would_overflow_saturate() {
        let tokens = Tokens {
            input: u64::MAX,
            output: u64::MAX,
            ..Tokens::default()
        };
        assert_eq!(tokens.total(), u64::MAX);
    }

    #[test]
    fn limits_know_when_they_say_nothing() {
        assert!(Limits::default().is_empty());
        assert!(
            !Limits {
                five_hour: Some(LimitWindow {
                    used_pct: 1.0,
                    resets_at: None
                }),
                weekly: None,
            }
            .is_empty()
        );
    }

    #[test]
    fn a_process_that_is_no_agent_matches_nothing() {
        assert!(adapter_for_process("bash", &["bash".to_owned()]).is_none());
        assert!(adapter_for_process("", &[]).is_none());
    }
}
