//! Claude Code: its session files, its screen, and how to resume it.
//!
//! Sessions are JSON Lines files under `~/.claude/projects/<encoded cwd>/`,
//! one per session, named after the session identifier. Each line is a record
//! with a `type`; otag reads the ones it understands and ignores the rest,
//! because the set of types grows with every release.
//!
//! The encoding of the directory name is lossy — every character that is not
//! a letter, a digit or a dash becomes a dash — so two directories can share
//! one encoded name. A session is therefore confirmed by the `cwd` its own
//! records carry, not by the directory it was found in.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Deserialize;

use crate::{
    AgentAdapter, AgentKind, Error, LimitWindow, Limits, PaneStatus, Result, SessionRef, Tokens,
    Transcript,
};

/// The program otag starts, and looks for in a pane.
pub const PROGRAM: &str = "claude";

/// Where Claude Code keeps its sessions, below the home directory.
const SESSIONS: [&str; 2] = [".claude", "projects"];

/// Where Claude Code keeps the settings otag merges its hooks into.
const SETTINGS: [&str; 2] = [".claude", "settings.json"];

/// Extension of a session file.
const SESSION_EXTENSION: &str = "jsonl";

/// Words the agent puts on screen while it is working.
///
/// Claude Code shows how to interrupt it for as long as it is busy, which is
/// the one part of its spinner that does not change with the phrasing.
const WORKING: [&str; 2] = ["esc to interrupt", "ctrl+b to run in background"];

/// Words the agent puts on screen when it wants permission.
const ASKING: [&str; 4] = [
    "Do you want to proceed?",
    "Do you want to make this edit",
    "Do you want to create",
    "No, and tell Claude what to do differently",
];

/// Words the agent puts on screen when something went wrong.
const FAILING: [&str; 2] = ["API Error", "Claude usage limit reached"];

/// The Claude Code adapter.
#[derive(Debug, Clone, Copy, Default)]
pub struct Claude;

impl AgentAdapter for Claude {
    fn kind(&self) -> AgentKind {
        AgentKind::Claude
    }

    fn matches_process(&self, exe: &str, argv: &[String]) -> bool {
        is_program(exe, argv, PROGRAM)
    }

    fn is_asking(&self, screen: &str) -> bool {
        ASKING.iter().any(|marker| screen.contains(marker))
    }

    fn answer_keys(&self, answer: crate::approval::Answer) -> Vec<u8> {
        // The prompt is a list whose first entry is the plain yes, and it
        // names `esc` as the way to refuse — "No, and tell Claude what to do
        // differently (esc)".
        match answer {
            crate::approval::Answer::Allow => crate::approval::FIRST_OPTION.to_vec(),
            crate::approval::Answer::Deny => crate::approval::ESCAPE.to_vec(),
        }
    }

    fn detect_status(&self, screen: &str) -> Option<PaneStatus> {
        // Asked first: a question is on screen while the agent is also
        // showing how to interrupt it, and the question is what matters.
        if ASKING.iter().any(|marker| screen.contains(marker)) {
            return Some(PaneStatus::NeedsApproval);
        }
        if FAILING.iter().any(|marker| screen.contains(marker)) {
            return Some(PaneStatus::Error);
        }
        if WORKING.iter().any(|marker| screen.contains(marker)) {
            return Some(PaneStatus::Working);
        }
        None
    }

    fn session_store(&self, home: &Path) -> PathBuf {
        SESSIONS
            .iter()
            .fold(home.to_path_buf(), |path, part| path.join(part))
    }

    fn find_session_for(
        &self,
        home: &Path,
        cwd: &Path,
        started_after: Option<SystemTime>,
    ) -> Option<SessionRef> {
        let directory = self.session_store(home).join(encode_cwd(cwd));
        let mut best: Option<(SystemTime, SessionRef)> = None;

        for entry in std::fs::read_dir(&directory).ok()?.flatten() {
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some(SESSION_EXTENSION)
            {
                continue;
            }
            let Ok(modified) = entry.metadata().and_then(|data| data.modified()) else {
                continue;
            };
            if started_after.is_some_and(|after| modified < after) {
                continue;
            }
            // The encoded directory name is lossy, so the session has to
            // agree about where it was working.
            let session = self.session_in(&path)?;
            if session
                .cwd
                .as_deref()
                .is_some_and(|recorded| recorded != cwd)
            {
                continue;
            }
            if best.as_ref().is_none_or(|(best, _)| modified > *best) {
                best = Some((modified, session));
            }
        }

        best.map(|(_, session)| session)
    }

    fn resume_command(&self, session: &SessionRef) -> Vec<String> {
        vec![
            PROGRAM.to_owned(),
            "--resume".to_owned(),
            session.id.clone(),
        ]
    }

    fn start_command(&self, initial_prompt: Option<&str>, model: Option<&str>) -> Vec<String> {
        let mut command = vec![PROGRAM.to_owned()];
        if let Some(model) = model.filter(|model| !model.is_empty()) {
            command.push("--model".to_owned());
            command.push(model.to_owned());
        }
        // The prompt goes last: it is an argument, not an option's value.
        command.extend(initial_prompt.map(str::to_owned));
        command
    }

    fn resume_on_model(&self, session: &SessionRef, model: &str) -> Vec<String> {
        let mut command = self.resume_command(session);
        command.push("--model".to_owned());
        command.push(model.to_owned());
        command
    }

    fn read_transcript(&self, path: &Path) -> Result<Transcript> {
        let text = std::fs::read_to_string(path).map_err(|source| Error::Unreadable {
            agent: AgentKind::Claude,
            path: path.to_path_buf(),
            source,
        })?;
        Ok(parse_transcript(&text, session_id_from_path(path)))
    }
}

impl Claude {
    /// What one session file says about itself, without reading all of it.
    fn session_in(&self, path: &Path) -> Option<SessionRef> {
        let id = session_id_from_path(path)?;
        let cwd = std::fs::read_to_string(path).ok().and_then(|text| {
            text.lines()
                .filter_map(|line| serde_json::from_str::<Record>(line).ok())
                .find_map(|record| record.cwd)
        });

        Some(SessionRef {
            id,
            path: path.to_path_buf(),
            cwd,
        })
    }
}

/// Whether a process is `program`, however it was started.
///
/// An agent may be run from a wrapper script, through a version manager, or
/// as an argument to an interpreter, so the name is looked for in the
/// program and in what it was started with — but only as a whole word, so
/// that a pane running an editor on a file called `claude` is not mistaken
/// for the agent.
pub(crate) fn is_program(exe: &str, argv: &[String], program: &str) -> bool {
    if file_name(exe) == program {
        return true;
    }
    let Some(first) = argv.first() else {
        return false;
    };
    if file_name(first) == program {
        return true;
    }
    // Installed from npm or behind a version manager, the agent is a script
    // with an interpreter in front of it: `node /usr/local/bin/claude`, or
    // `/bin/sh /opt/shims/claude` for a shim. The script is the agent, so
    // the interpreter's first non-flag argument is looked at too — and only
    // that one, so that a file given to the agent later on the command line
    // cannot be mistaken for it.
    if !INTERPRETERS.contains(&file_name(first)) {
        return false;
    }
    argv.iter()
        .skip(1)
        .find(|argument| !argument.starts_with('-'))
        .is_some_and(|script| file_name(script) == program)
}

/// Programs that run an agent rather than being one.
///
/// The shells are in the list because a version manager installs the agent
/// as a shim: a short shell script that eventually runs the real program.
const INTERPRETERS: [&str; 10] = [
    "node", "nodejs", "bun", "deno", "python", "python3", "sh", "bash", "zsh", "dash",
];

/// The last part of a path, which is the program's own name.
fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// The directory name Claude Code keeps a session under.
///
/// Every character that is not a letter, a digit or a dash becomes a dash,
/// which is what the agent itself does.
#[must_use]
pub fn encode_cwd(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' {
                character
            } else {
                '-'
            }
        })
        .collect()
}

/// Where Claude Code keeps the settings otag merges its hooks into.
#[must_use]
pub fn settings_path(home: &Path) -> PathBuf {
    SETTINGS
        .iter()
        .fold(home.to_path_buf(), |path, part| path.join(part))
}

/// The session identifier a file is named after.
fn session_id_from_path(path: &Path) -> Option<String> {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|stem| !stem.is_empty())
}

/// One line of a session file, as far as otag reads it.
#[derive(Debug, Deserialize)]
struct Record {
    #[serde(default)]
    r#type: Option<String>,
    #[serde(default)]
    cwd: Option<PathBuf>,
    #[serde(default, alias = "sessionId")]
    session_id: Option<String>,
    #[serde(default, rename = "gitBranch")]
    git_branch: Option<String>,
    #[serde(default)]
    message: Option<Message>,
}

/// What an agent or a user said.
#[derive(Debug, Deserialize)]
struct Message {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    content: Option<Content>,
    #[serde(default)]
    usage: Option<Usage>,
}

/// A message's content, which is either plain text or a list of blocks.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Content {
    /// What the user typed.
    Text(String),
    /// What the agent produced, block by block.
    Blocks(Vec<Block>),
}

/// One part of an agent's answer.
#[derive(Debug, Deserialize)]
struct Block {
    #[serde(default)]
    r#type: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    input: Option<ToolInput>,
}

/// The part of a tool call otag reads: what it touched, and what it ran.
#[derive(Debug, Deserialize)]
struct ToolInput {
    #[serde(default)]
    file_path: Option<PathBuf>,
    #[serde(default)]
    command: Option<String>,
}

/// What a turn cost.
#[derive(Debug, Default, Deserialize)]
struct Usage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
}

impl Usage {
    /// Everything the model was sent, which is what a context meter shows.
    const fn context(&self) -> u64 {
        self.input_tokens
            .saturating_add(self.cache_read_input_tokens)
            .saturating_add(self.cache_creation_input_tokens)
    }
}

/// Tools whose calls mean a file was written to.
const WRITING_TOOLS: [&str; 4] = ["Edit", "Write", "MultiEdit", "NotebookEdit"];

/// Read a whole session file.
///
/// A line that cannot be parsed is skipped: a file being written while it is
/// read ends in half a line, and a release that adds a record type must not
/// make the rest of the file unreadable.
#[must_use]
pub fn parse_transcript(text: &str, session_id: Option<String>) -> Transcript {
    let mut transcript = Transcript {
        session_id,
        ..Transcript::default()
    };
    let mut files = BTreeSet::new();

    for line in text.lines() {
        let Ok(record) = serde_json::from_str::<Record>(line) else {
            continue;
        };
        if transcript.session_id.is_none() {
            transcript.session_id = record.session_id;
        }
        if let Some(cwd) = record.cwd {
            transcript.cwd = Some(cwd);
        }
        if let Some(branch) = record.git_branch.filter(|branch| !branch.is_empty()) {
            transcript.git_branch = Some(branch);
        }

        let Some(message) = record.message else {
            continue;
        };
        match record.r#type.as_deref() {
            Some("user") => {
                if let Some(Content::Text(text)) = message.content
                    && !text.trim().is_empty()
                {
                    transcript.prompts.push(text);
                }
            }
            Some("assistant") => {
                transcript.turns += 1;
                if let Some(model) = message.model {
                    transcript.model = Some(model);
                }
                if let Some(usage) = message.usage {
                    add_usage(&mut transcript.tokens, &usage);
                }
                if let Some(Content::Blocks(blocks)) = message.content {
                    read_blocks(&mut transcript, &mut files, blocks);
                }
            }
            _ => {}
        }
    }

    transcript.files_touched = files.into_iter().collect();
    transcript
}

/// Add one turn's cost to the total, and make it the context size.
fn add_usage(tokens: &mut Tokens, usage: &Usage) {
    tokens.input = tokens.input.saturating_add(usage.input_tokens);
    tokens.output = tokens.output.saturating_add(usage.output_tokens);
    tokens.cache_read = tokens
        .cache_read
        .saturating_add(usage.cache_read_input_tokens);
    tokens.cache_written = tokens
        .cache_written
        .saturating_add(usage.cache_creation_input_tokens);
    // The newest turn is the one whose context the user is looking at.
    tokens.context = usage.context();
}

/// Read what otag needs out of one answer's blocks.
fn read_blocks(transcript: &mut Transcript, files: &mut BTreeSet<PathBuf>, blocks: Vec<Block>) {
    for block in blocks {
        match block.r#type.as_deref() {
            Some("text") => {
                if let Some(text) = block.text.filter(|text| !text.trim().is_empty()) {
                    transcript.last_message = Some(text);
                }
            }
            Some("tool_use") => {
                let Some(input) = block.input else { continue };
                let name = block.name.unwrap_or_default();
                if WRITING_TOOLS.contains(&name.as_str())
                    && let Some(path) = input.file_path
                {
                    files.insert(path);
                }
                if name == "Bash"
                    && let Some(command) = input.command
                {
                    transcript.commands.push(command);
                }
            }
            _ => {}
        }
    }
}

/// What Claude Code pushes to a statusLine command on its standard input.
///
/// Only a Pro or Max account is sent `rate_limits`; an account using an API
/// key is sent none, and neither is anyone before the first answer of a
/// session. Every field is therefore optional.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct StatusLine {
    /// The session the usage belongs to.
    #[serde(default)]
    pub session_id: Option<String>,
    /// Where the agent is working.
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    /// The file the session is kept in.
    #[serde(default)]
    pub transcript_path: Option<PathBuf>,
    /// Which model is answering.
    #[serde(default)]
    pub model: Option<Model>,
    /// How much of the account's limits is used.
    #[serde(default)]
    pub rate_limits: Option<RateLimits>,
    /// What the agent says about the context it is using.
    ///
    /// Kept as it arrived: otag has no verified sample of this field's shape,
    /// and a guess at it would be worse than passing it on untouched.
    #[serde(default)]
    pub context_window: Option<serde_json::Value>,
}

/// The model a session is using.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Model {
    /// Its identifier.
    #[serde(default)]
    pub id: Option<String>,
    /// Its name, as the agent shows it.
    #[serde(default)]
    pub display_name: Option<String>,
}

/// How much of an account's limits is used.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct RateLimits {
    /// The five-hour window.
    #[serde(default)]
    pub five_hour: Option<Window>,
    /// The seven-day window.
    #[serde(default)]
    pub seven_day: Option<Window>,
}

/// One usage window.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct Window {
    /// How much of it is used, as a percentage.
    #[serde(default)]
    pub used_percentage: Option<f32>,
    /// When it resets, in seconds since the epoch.
    #[serde(default)]
    pub resets_at: Option<i64>,
}

impl StatusLine {
    /// Read what the agent sent, or nothing when it is not JSON at all.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        serde_json::from_str(text).ok()
    }

    /// The usage windows, in the shape the rest of otag uses.
    #[must_use]
    pub fn limits(&self) -> Limits {
        let Some(limits) = self.rate_limits else {
            return Limits::default();
        };
        Limits {
            five_hour: limits.five_hour.and_then(window),
            weekly: limits.seven_day.and_then(window),
        }
    }
}

/// One window, if the agent said how much of it is used.
fn window(window: Window) -> Option<LimitWindow> {
    Some(LimitWindow {
        used_pct: window.used_percentage?,
        resets_at: window.resets_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_agent_run_by_an_interpreter_is_still_the_agent() {
        // How an npm install of the agent appears in a process list.
        let argv = ["node".to_owned(), "/usr/local/bin/claude".to_owned()];
        assert!(is_program("node", &argv, PROGRAM));
    }

    #[test]
    fn an_interpreter_given_a_flag_before_the_script_still_matches() {
        let argv = [
            "node".to_owned(),
            "--enable-source-maps".to_owned(),
            "/opt/claude".to_owned(),
        ];
        assert!(is_program("node", &argv, PROGRAM));
    }

    #[test]
    fn an_agent_behind_a_shell_shim_is_still_the_agent() {
        let argv = ["/bin/sh".to_owned(), "/opt/shims/claude".to_owned()];
        assert!(is_program("/bin/sh", &argv, PROGRAM));
    }

    #[test]
    fn a_login_shell_is_not_an_agent() {
        let argv = ["-zsh".to_owned()];
        assert!(!is_program("-zsh", &argv, PROGRAM));
    }

    #[test]
    fn a_file_named_after_the_agent_is_not_the_agent() {
        let argv = [
            "node".to_owned(),
            "build.js".to_owned(),
            "claude".to_owned(),
        ];
        assert!(!is_program("node", &argv, PROGRAM));
        let argv = ["vim".to_owned(), "claude".to_owned()];
        assert!(!is_program("vim", &argv, PROGRAM));
    }

    fn fixture(name: &str) -> String {
        let path = Path::new(crate::FIXTURES).join(name);
        std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("the fixture {} is readable: {error}", path.display()))
    }

    #[test]
    fn a_pane_running_the_agent_is_recognised() {
        let claude = Claude;
        assert!(claude.matches_process("claude", &["claude".to_owned()]));
        assert!(claude.matches_process(
            "/usr/local/bin/claude",
            &["/usr/local/bin/claude".to_owned(), "--resume".to_owned()]
        ));
        assert!(
            claude.matches_process("node", &["/opt/homebrew/bin/claude".to_owned()]),
            "an agent started through an interpreter is still the agent"
        );
    }

    #[test]
    fn a_pane_running_something_else_is_not_the_agent() {
        let claude = Claude;
        assert!(!claude.matches_process("bash", &["bash".to_owned()]));
        assert!(
            !claude.matches_process("vim", &["vim".to_owned(), "claude".to_owned()]),
            "editing a file called claude is not running the agent"
        );
        assert!(!claude.matches_process("claude-helper", &["claude-helper".to_owned()]));
        assert!(!claude.matches_process("", &[]));
    }

    #[test]
    fn a_directory_is_encoded_the_way_the_agent_encodes_it() {
        assert_eq!(
            encode_cwd(Path::new("/projects/my-app")),
            "-projects-my-app"
        );
        // Anything that is not a letter, a digit or a dash becomes a dash,
        // which is why the encoding cannot be undone.
        assert_eq!(
            encode_cwd(Path::new("/projects/neweditör")),
            "-projects-newedit-r"
        );
        assert_eq!(encode_cwd(Path::new("/a.b/c_d")), "-a-b-c-d");
        assert_eq!(encode_cwd(Path::new("/")), "-");
    }

    #[test]
    fn the_commands_that_start_and_resume_the_agent() {
        let claude = Claude;
        let session = SessionRef {
            id: "0199c7d4-0e5f-7281-b418-fe1e2ccaf5a3".to_owned(),
            path: PathBuf::from("/projects/app/session.jsonl"),
            cwd: None,
        };

        assert_eq!(
            claude.resume_command(&session),
            vec!["claude", "--resume", &session.id]
        );
        assert_eq!(
            claude.resume_on_model(&session, "sonnet"),
            vec!["claude", "--resume", &session.id, "--model", "sonnet"]
        );
        assert_eq!(claude.start_command(None, None), vec!["claude"]);
        assert_eq!(
            claude.start_command(Some("go"), Some("sonnet")),
            vec!["claude", "--model", "sonnet", "go"],
            "the model is an option and the prompt an argument"
        );
        assert_eq!(
            claude.start_command(None, Some("")),
            vec!["claude"],
            "an empty model is no model"
        );
        assert_eq!(
            claude.start_command(Some("read the handoff"), None),
            vec!["claude", "read the handoff"]
        );
    }

    #[test]
    fn the_screen_says_when_the_agent_is_working() {
        let claude = Claude;
        assert_eq!(
            claude.detect_status("\u{273b} Thinking… (12s · esc to interrupt)"),
            Some(PaneStatus::Working)
        );
        assert_eq!(
            claude.detect_status("Running… (ctrl+b to run in background)"),
            Some(PaneStatus::Working)
        );
    }

    #[test]
    fn the_screen_says_when_the_agent_wants_permission() {
        let claude = Claude;
        let screen = "Bash command\n\n  rm -rf build\n\nDo you want to proceed?\n\
                      \u{276f} 1. Yes\n  2. No, and tell Claude what to do differently";
        assert_eq!(
            claude.detect_status(screen),
            Some(PaneStatus::NeedsApproval)
        );
    }

    #[test]
    fn a_question_counts_for_more_than_a_spinner() {
        // Both can be on screen at once, and the question is what the user
        // has to act on.
        let claude = Claude;
        let screen = "Do you want to proceed?\n\u{273b} Waiting… (esc to interrupt)";
        assert_eq!(
            claude.detect_status(screen),
            Some(PaneStatus::NeedsApproval)
        );
    }

    #[test]
    fn the_screen_says_when_the_agent_has_a_problem() {
        let claude = Claude;
        assert_eq!(
            claude.detect_status("API Error: 500 internal server error"),
            Some(PaneStatus::Error)
        );
        assert_eq!(
            claude.detect_status("Claude usage limit reached. Your limit will reset at 3pm."),
            Some(PaneStatus::Error)
        );
    }

    #[test]
    fn a_screen_the_adapter_does_not_recognise_is_not_a_guess() {
        // The words an agent shows change with every release; a wrong status
        // is worse than none, because hooks report the truth anyway.
        let claude = Claude;
        assert_eq!(claude.detect_status(""), None);
        assert_eq!(claude.detect_status("$ ls -la\nREADME.md"), None);
    }

    #[test]
    fn a_transcript_is_read_into_the_shape_otag_uses() {
        let transcript = parse_transcript(&fixture("claude-transcript.jsonl"), None);

        assert_eq!(
            transcript.session_id.as_deref(),
            Some("11111111-2222-3333-4444-555555555555")
        );
        assert_eq!(transcript.cwd, Some(PathBuf::from("/projects/app")));
        assert_eq!(transcript.git_branch.as_deref(), Some("main"));
        assert_eq!(transcript.model.as_deref(), Some("claude-opus-5"));
        assert_eq!(transcript.turns, 2);
        assert_eq!(
            transcript.prompts,
            vec![
                "add a test for the parser".to_owned(),
                "now run it".to_owned()
            ]
        );
        assert_eq!(transcript.last_message.as_deref(), Some("The test passes."));
        assert_eq!(
            transcript.files_touched,
            vec![PathBuf::from("/projects/app/src/parser.rs")]
        );
        assert_eq!(transcript.commands, vec!["cargo test".to_owned()]);
    }

    #[test]
    fn a_transcript_adds_up_the_tokens_and_keeps_the_last_context() {
        let transcript = parse_transcript(&fixture("claude-transcript.jsonl"), None);
        let tokens = transcript.tokens;

        assert_eq!(tokens.input, 12);
        assert_eq!(tokens.output, 400);
        assert_eq!(tokens.cache_read, 20_000);
        assert_eq!(tokens.cache_written, 30_000);
        assert_eq!(
            tokens.context, 20_002,
            "the context is the newest turn's, not the total"
        );
    }

    #[test]
    fn a_transcript_line_that_cannot_be_read_does_not_hide_the_rest() {
        // A file being written ends in half a line, and a new release adds
        // record types otag has never seen.
        let text = "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"first\"}}\n\
                    not json at all\n\
                    {\"type\":\"from-the-future\",\"shape\":{\"nobody\":\"knows\"}}\n\
                    {\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"second\"}}\n\
                    {\"type\":\"user\",\"message\":{\"role\":\"user\",\"cont";

        let transcript = parse_transcript(text, None);
        assert_eq!(
            transcript.prompts,
            vec!["first".to_owned(), "second".to_owned()]
        );
    }

    #[test]
    fn an_empty_transcript_reads_as_nothing_rather_than_failing() {
        let transcript = parse_transcript("", Some("an-id".to_owned()));
        assert_eq!(transcript.session_id.as_deref(), Some("an-id"));
        assert_eq!(transcript.turns, 0);
        assert!(transcript.prompts.is_empty());
        assert_eq!(transcript.tokens, Tokens::default());
    }

    #[test]
    fn a_session_file_that_is_not_there_is_an_error_that_names_it() {
        let error = Claude
            .read_transcript(Path::new("/no/such/session.jsonl"))
            .expect_err("there is no such file");
        assert!(
            error.to_string().contains("/no/such/session.jsonl"),
            "{error}"
        );
        assert!(error.to_string().contains("claude"), "{error}");
    }

    #[test]
    fn the_newest_session_for_a_directory_is_the_one_found() {
        let home = tempfile::tempdir().expect("a temporary home");
        let cwd = Path::new("/projects/app");
        let directory = Claude.session_store(home.path()).join(encode_cwd(cwd));
        std::fs::create_dir_all(&directory).expect("the session directory");

        let line = |cwd: &str| {
            format!("{{\"type\":\"user\",\"cwd\":\"{cwd}\",\"message\":{{\"content\":\"hi\"}}}}\n")
        };
        std::fs::write(directory.join("older.jsonl"), line("/projects/app"))
            .expect("the older session");
        // Written second, so it is the newer one whatever the clock does.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(directory.join("newer.jsonl"), line("/projects/app"))
            .expect("the newer session");
        std::fs::write(directory.join("notes.txt"), "not a session")
            .expect("something that is not a session");

        let session = Claude
            .find_session_for(home.path(), cwd, None)
            .expect("a session is found");
        assert_eq!(session.id, "newer");
        assert_eq!(session.cwd.as_deref(), Some(cwd));
    }

    #[test]
    fn a_session_from_another_directory_with_the_same_encoded_name_is_not_used() {
        // The encoding is lossy: `/projects/a.b` and `/projects/a-b` share a
        // directory, and resuming the wrong one would put an agent to work
        // in a project the user did not ask about.
        let home = tempfile::tempdir().expect("a temporary home");
        let wanted = Path::new("/projects/a-b");
        let other = "/projects/a.b";
        assert_eq!(encode_cwd(wanted), encode_cwd(Path::new(other)));

        let directory = Claude.session_store(home.path()).join(encode_cwd(wanted));
        std::fs::create_dir_all(&directory).expect("the session directory");
        std::fs::write(
            directory.join("theirs.jsonl"),
            format!("{{\"type\":\"user\",\"cwd\":\"{other}\"}}\n"),
        )
        .expect("the other session");

        assert_eq!(Claude.find_session_for(home.path(), wanted, None), None);
    }

    #[test]
    fn a_session_older_than_the_pane_asking_is_not_adopted() {
        let home = tempfile::tempdir().expect("a temporary home");
        let cwd = Path::new("/projects/app");
        let directory = Claude.session_store(home.path()).join(encode_cwd(cwd));
        std::fs::create_dir_all(&directory).expect("the session directory");
        std::fs::write(directory.join("earlier.jsonl"), "{\"type\":\"user\"}\n")
            .expect("the session");

        let later = SystemTime::now() + std::time::Duration::from_secs(60);
        assert_eq!(Claude.find_session_for(home.path(), cwd, Some(later)), None);
        assert!(
            Claude
                .find_session_for(home.path(), cwd, Some(SystemTime::UNIX_EPOCH))
                .is_some()
        );
    }

    #[test]
    fn a_directory_with_no_sessions_yields_nothing() {
        let home = tempfile::tempdir().expect("a temporary home");
        assert_eq!(
            Claude.find_session_for(home.path(), Path::new("/projects/app"), None),
            None
        );
    }

    #[test]
    fn the_status_line_says_how_much_of_the_limits_is_used() {
        let status =
            StatusLine::parse(&fixture("claude-statusline.json")).expect("the status line parses");

        assert_eq!(
            status.session_id.as_deref(),
            Some("11111111-2222-3333-4444-555555555555")
        );
        assert_eq!(
            status
                .model
                .as_ref()
                .and_then(|model| model.display_name.as_deref()),
            Some("Opus 5")
        );

        let limits = status.limits();
        let five_hour = limits.five_hour.expect("the five-hour window");
        assert!((five_hour.used_pct - 70.0).abs() < f32::EPSILON);
        assert_eq!(five_hour.resets_at, Some(1_793_660_400));
        let weekly = limits.weekly.expect("the weekly window");
        assert!((weekly.used_pct - 58.0).abs() < 0.001);
    }

    #[test]
    fn a_status_line_without_any_usage_in_it_is_still_read() {
        // An account using an API key is sent no usage at all, and nobody is
        // sent any before the first answer of a session.
        let status = StatusLine::parse(&fixture("claude-statusline-no-usage.json"))
            .expect("the status line parses");
        assert!(status.limits().is_empty());
        assert_eq!(status.cwd, Some(PathBuf::from("/projects/app")));
    }

    #[test]
    fn a_window_without_a_percentage_is_not_a_window() {
        let status = StatusLine::parse(r#"{"rate_limits":{"five_hour":{"resets_at":12}}}"#)
            .expect("it parses");
        assert!(status.limits().is_empty());
    }

    #[test]
    fn something_that_is_not_a_status_line_is_not_read_as_one() {
        assert!(StatusLine::parse("").is_none());
        assert!(StatusLine::parse("not json at all").is_none());
        // Anything unknown in it is ignored rather than refused.
        assert!(StatusLine::parse(r#"{"from_the_future":true}"#).is_some());
    }
}
