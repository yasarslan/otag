//! Codex CLI: its rollout files, its screen, and how to resume it.
//!
//! A session is a JSON Lines "rollout" under
//! `~/.codex/sessions/YYYY/MM/DD/rollout-<timestamp>-<id>.jsonl`. Every line
//! has a `type` and a `payload`; otag reads `session_meta` for what the
//! session is, `turn_context` for the model and the directory, and
//! `token_usage_record` and the `token_count` event for what it has cost —
//! including the account's rate limits, which the agent writes into its own
//! file and no other source has.
//!
//! Codex needs no hooks: the daemon watches the newest rollout and reads the
//! lines as they are appended.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Deserialize;

use crate::claude::is_program;
use crate::{
    AgentAdapter, AgentKind, Error, LimitWindow, Limits, PaneStatus, Result, SessionRef, Tokens,
    Transcript,
};

/// The program otag starts, and looks for in a pane.
pub const PROGRAM: &str = "codex";

/// Where Codex keeps its sessions, below the home directory.
const SESSIONS: [&str; 2] = [".codex", "sessions"];

/// Extension of a rollout file.
const ROLLOUT_EXTENSION: &str = "jsonl";

/// How deep the dated directories go: year, month, day.
const DATE_DEPTH: usize = 3;

/// Words the agent puts on screen while it is working.
///
/// Unverified against a running Codex on the machine otag was built on: the
/// agent was not installed. They are the words its interface is known to use,
/// and a screen matching none of them gives no status at all rather than a
/// wrong one.
const WORKING: [&str; 2] = ["Esc to interrupt", "esc to interrupt"];

/// Words the agent puts on screen when it wants permission.
const ASKING: [&str; 3] = [
    "Allow command?",
    "Would you like to proceed?",
    "approval required",
];

/// Words the agent puts on screen when something went wrong.
const FAILING: [&str; 2] = ["stream error", "You've hit your usage limit"];

/// The Codex CLI adapter.
#[derive(Debug, Clone, Copy, Default)]
pub struct Codex;

impl AgentAdapter for Codex {
    fn kind(&self) -> AgentKind {
        AgentKind::Codex
    }

    fn matches_process(&self, exe: &str, argv: &[String]) -> bool {
        is_program(exe, argv, PROGRAM)
    }

    fn is_asking(&self, screen: &str) -> bool {
        ASKING.iter().any(|marker| screen.contains(marker))
    }

    fn answer_keys(&self, answer: crate::approval::Answer) -> Vec<u8> {
        // Unverified against a running Codex, like the words above: its
        // prompt is a list whose first entry is the plain yes, and `esc`
        // refuses. Nothing is typed unless the screen is asking, and the
        // sender checks afterwards that the prompt is gone.
        match answer {
            crate::approval::Answer::Allow => crate::approval::FIRST_OPTION.to_vec(),
            crate::approval::Answer::Deny => crate::approval::ESCAPE.to_vec(),
        }
    }

    fn detect_status(&self, screen: &str) -> Option<PaneStatus> {
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
        let mut best: Option<(SystemTime, SessionRef)> = None;

        for path in rollouts(&self.session_store(home)) {
            let Ok(modified) = path.metadata().and_then(|data| data.modified()) else {
                continue;
            };
            if started_after.is_some_and(|after| modified < after) {
                continue;
            }
            let Some(session) = self.session_in(&path) else {
                continue;
            };
            if session.cwd.as_deref() != Some(cwd) {
                continue;
            }
            if best.as_ref().is_none_or(|(best, _)| modified > *best) {
                best = Some((modified, session));
            }
        }

        best.map(|(_, session)| session)
    }

    fn resume_command(&self, session: &SessionRef) -> Vec<String> {
        vec![PROGRAM.to_owned(), "resume".to_owned(), session.id.clone()]
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
            agent: AgentKind::Codex,
            path: path.to_path_buf(),
            source,
        })?;
        Ok(parse_rollout(&text, session_id_from_path(path)))
    }
}

impl Codex {
    /// What one rollout says about itself, read from its first lines.
    fn session_in(&self, path: &Path) -> Option<SessionRef> {
        let text = std::fs::read_to_string(path).ok()?;
        let mut id = session_id_from_path(path);
        let mut cwd = None;

        for line in text.lines() {
            let Ok(record) = serde_json::from_str::<Record>(line) else {
                continue;
            };
            let Some(payload) = record.payload else {
                continue;
            };
            if id.is_none() {
                id = payload.id.or(payload.session_id);
            }
            if let Some(found) = payload.cwd {
                cwd = Some(found);
                break;
            }
        }

        Some(SessionRef {
            id: id?,
            path: path.to_path_buf(),
            cwd,
        })
    }
}

/// Every rollout below a sessions directory, in no particular order.
///
/// The files are three directories deep, one level each for the year, the
/// month and the day. Walking only that far keeps otag out of anything else
/// the agent may keep there.
fn rollouts(sessions: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut directories = vec![(sessions.to_path_buf(), 0_usize)];

    while let Some((directory, depth)) = directories.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if depth < DATE_DEPTH {
                    directories.push((path, depth + 1));
                }
            } else if path.extension().and_then(|extension| extension.to_str())
                == Some(ROLLOUT_EXTENSION)
            {
                found.push(path);
            }
        }
    }
    found
}

/// The session identifier in a rollout's file name.
///
/// The name is `rollout-<timestamp>-<id>`, and the identifier is a UUID, so
/// it is the last five dash-separated parts of the name.
fn session_id_from_path(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    let parts: Vec<&str> = stem.split('-').collect();
    if parts.len() < 5 {
        return None;
    }
    let id = parts[parts.len() - 5..].join("-");
    looks_like_a_uuid(&id).then_some(id)
}

/// Whether a string has the shape of a UUID.
fn looks_like_a_uuid(text: &str) -> bool {
    let groups = [8, 4, 4, 4, 12];
    let parts: Vec<&str> = text.split('-').collect();
    parts.len() == groups.len()
        && parts.iter().zip(groups).all(|(part, length)| {
            part.len() == length && part.chars().all(|c| c.is_ascii_hexdigit())
        })
}

/// One line of a rollout, as far as otag reads it.
#[derive(Debug, Deserialize)]
struct Record {
    #[serde(default)]
    r#type: Option<String>,
    #[serde(default)]
    payload: Option<Payload>,
}

/// Everything otag reads out of any record's payload.
///
/// One shape for every record type, because the fields otag wants appear in
/// several of them and a record that carries none is simply empty.
#[derive(Debug, Deserialize)]
struct Payload {
    #[serde(default)]
    r#type: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    cwd: Option<PathBuf>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    git: Option<Git>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    content: Option<Vec<ContentPart>>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
    #[serde(default)]
    usage: Option<Usage>,
    #[serde(default)]
    thread_token_usage: Option<Usage>,
    #[serde(default)]
    info: Option<Info>,
    #[serde(default)]
    rate_limits: Option<RateLimits>,
}

/// What the session knew about the checkout it was in.
#[derive(Debug, Deserialize)]
struct Git {
    #[serde(default)]
    branch: Option<String>,
}

/// One part of a message.
#[derive(Debug, Deserialize)]
struct ContentPart {
    #[serde(default)]
    text: Option<String>,
}

/// What a turn, or a whole session, has cost.
#[derive(Debug, Default, Clone, Copy, Deserialize)]
struct Usage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    cached_input_tokens: u64,
    #[serde(default)]
    cache_write_input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
}

/// What the agent reports alongside a token count.
#[derive(Debug, Deserialize)]
struct Info {
    #[serde(default)]
    total_token_usage: Option<Usage>,
    #[serde(default)]
    last_token_usage: Option<Usage>,
    #[serde(default)]
    model_context_window: Option<u64>,
}

/// How much of the account's limits is used.
#[derive(Debug, Deserialize)]
struct RateLimits {
    /// The shorter window; `window_minutes` says how long it is.
    #[serde(default)]
    primary: Option<Window>,
    /// The longer window.
    #[serde(default)]
    secondary: Option<Window>,
}

/// One usage window.
#[derive(Debug, Clone, Copy, Deserialize)]
struct Window {
    #[serde(default)]
    used_percent: Option<f32>,
    #[serde(default)]
    window_minutes: Option<u32>,
    #[serde(default)]
    resets_at: Option<i64>,
}

/// Longest window otag counts as the short one, in minutes.
///
/// The agent says how long each window is rather than naming it, so the
/// windows are sorted by length: anything up to a day is the short one.
const SHORT_WINDOW_MINUTES: u32 = 24 * 60;

/// Tool calls that run a shell command.
const SHELL_TOOLS: [&str; 3] = ["shell", "container.exec", "local_shell"];

/// Read a whole rollout file.
///
/// A line that cannot be parsed is skipped: the file is appended to while it
/// is read, so the last line is often half written.
#[must_use]
pub fn parse_rollout(text: &str, session_id: Option<String>) -> Transcript {
    let mut transcript = Transcript {
        session_id,
        ..Transcript::default()
    };
    let mut commands = BTreeSet::new();

    for line in text.lines() {
        let Ok(record) = serde_json::from_str::<Record>(line) else {
            continue;
        };
        let Some(payload) = record.payload else {
            continue;
        };

        if transcript.session_id.is_none() {
            transcript.session_id = payload.id.clone().or_else(|| payload.session_id.clone());
        }
        if let Some(cwd) = payload.cwd.clone() {
            transcript.cwd = Some(cwd);
        }
        if let Some(model) = payload.model.clone() {
            transcript.model = Some(model);
        }
        if let Some(branch) = payload.git.as_ref().and_then(|git| git.branch.clone()) {
            transcript.git_branch = Some(branch);
        }

        match record.r#type.as_deref() {
            Some("response_item") => read_item(&mut transcript, &mut commands, &payload),
            Some("token_usage_record") => {
                // The session's own running total, so it replaces rather
                // than adds to what was read before.
                if let Some(usage) = payload.thread_token_usage.or(payload.usage) {
                    transcript.tokens = tokens(&usage, transcript.tokens.context);
                }
            }
            Some("event_msg") => read_event(&mut transcript, &payload),
            _ => {}
        }
    }

    transcript.commands = commands.into_iter().collect();
    transcript
}

/// Read one of the items a turn produced.
fn read_item(transcript: &mut Transcript, commands: &mut BTreeSet<String>, payload: &Payload) {
    match payload.r#type.as_deref() {
        Some("message") => {
            let text = payload
                .content
                .iter()
                .flatten()
                .filter_map(|part| part.text.as_deref())
                .collect::<Vec<_>>()
                .join("");
            if text.trim().is_empty() {
                return;
            }
            match payload.role.as_deref() {
                Some("user") => transcript.prompts.push(text),
                Some("assistant") => {
                    transcript.turns += 1;
                    transcript.last_message = Some(text);
                }
                // A rollout also carries the instructions the agent was
                // given, which are nobody's prompt and nobody's answer.
                _ => {}
            }
        }
        Some("function_call") => {
            let Some(name) = payload.name.as_deref() else {
                return;
            };
            if !SHELL_TOOLS.contains(&name) {
                return;
            }
            if let Some(command) = payload.arguments.as_deref().and_then(shell_command) {
                commands.insert(command);
            }
        }
        _ => {}
    }
}

/// Read one of the events the agent reported.
fn read_event(transcript: &mut Transcript, payload: &Payload) {
    if payload.r#type.as_deref() != Some("token_count") {
        return;
    }
    if let Some(info) = &payload.info {
        if let Some(total) = info.total_token_usage {
            // The agent counts the cached part inside its input tokens, so
            // the input count is the whole context the turn was sent.
            let context = info
                .last_token_usage
                .map_or(transcript.tokens.context, |last| last.input_tokens);
            transcript.tokens = tokens(&total, context);
        }
        if let Some(window) = info.model_context_window {
            transcript.context_window = Some(window);
        }
    }
    if let Some(limits) = &payload.rate_limits {
        transcript.limits = read_limits(limits);
    }
}

/// Turn the agent's token counts into otag's.
fn tokens(usage: &Usage, context: u64) -> Tokens {
    Tokens {
        // The agent reports the cached part inside the input count, so the
        // two have to be separated to mean the same thing everywhere.
        input: usage.input_tokens.saturating_sub(usage.cached_input_tokens),
        output: usage.output_tokens,
        cache_read: usage.cached_input_tokens,
        cache_written: usage.cache_write_input_tokens,
        context,
    }
}

/// Sort the agent's two windows into the short one and the long one.
fn read_limits(limits: &RateLimits) -> Limits {
    let mut short = None;
    let mut long = None;

    for window in [limits.primary, limits.secondary].into_iter().flatten() {
        let Some(used_pct) = window.used_percent else {
            continue;
        };
        let found = LimitWindow {
            used_pct,
            resets_at: window.resets_at,
        };
        if window
            .window_minutes
            .is_some_and(|minutes| minutes > SHORT_WINDOW_MINUTES)
        {
            long = Some(found);
        } else {
            short = Some(found);
        }
    }

    Limits {
        five_hour: short,
        weekly: long,
    }
}

/// The command a shell tool call was given.
///
/// The arguments arrive as JSON inside a string, and the command itself is
/// either a list of words or one line.
fn shell_command(arguments: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Arguments {
        #[serde(default)]
        command: Option<Command>,
    }
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Command {
        Line(String),
        Words(Vec<String>),
    }

    let parsed: Arguments = serde_json::from_str(arguments).ok()?;
    match parsed.command? {
        Command::Line(line) => Some(line),
        // A command run through a shell is the script it was given; the
        // shell in front of it says nothing about what happened.
        Command::Words(words) => match words.as_slice() {
            [shell, flag, script, ..]
                if SHELLS.contains(&shell.rsplit('/').next().unwrap_or(shell))
                    && SHELL_FLAGS.contains(&flag.as_str()) =>
            {
                Some(script.clone())
            }
            words => Some(words.join(" ")),
        },
    }
}

/// Shells a tool call may run a script through.
const SHELLS: [&str; 4] = ["bash", "sh", "zsh", "dash"];

/// The flags those shells take a script with.
const SHELL_FLAGS: [&str; 3] = ["-lc", "-c", "-ic"];

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        let path = Path::new(crate::FIXTURES).join(name);
        std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("the fixture {} is readable: {error}", path.display()))
    }

    #[test]
    fn a_pane_running_the_agent_is_recognised() {
        let codex = Codex;
        assert!(codex.matches_process("codex", &["codex".to_owned()]));
        assert!(codex.matches_process(
            "/opt/homebrew/bin/codex",
            &["/opt/homebrew/bin/codex".to_owned(), "resume".to_owned()]
        ));
        assert!(!codex.matches_process("bash", &["bash".to_owned()]));
        assert!(!codex.matches_process("codex-helper", &["codex-helper".to_owned()]));
    }

    #[test]
    fn the_commands_that_start_and_resume_the_agent() {
        let codex = Codex;
        let session = SessionRef {
            id: "019fc2ec-4d09-7d31-a2b1-e9f95a59a39b".to_owned(),
            path: PathBuf::from("/projects/app/rollout.jsonl"),
            cwd: None,
        };

        assert_eq!(
            codex.resume_command(&session),
            vec!["codex", "resume", &session.id]
        );
        assert_eq!(codex.start_command(None, None), vec!["codex"]);
        assert_eq!(
            codex.start_command(Some("go"), Some("o3")),
            vec!["codex", "--model", "o3", "go"]
        );
        assert_eq!(
            codex.start_command(Some("read the handoff"), None),
            vec!["codex", "read the handoff"]
        );
    }

    #[test]
    fn the_screen_says_what_the_agent_is_doing() {
        let codex = Codex;
        assert_eq!(
            codex.detect_status("Working (4s \u{2022} Esc to interrupt)"),
            Some(PaneStatus::Working)
        );
        assert_eq!(
            codex.detect_status("Allow command?\n  rm -rf build"),
            Some(PaneStatus::NeedsApproval)
        );
        assert_eq!(
            codex.detect_status("You've hit your usage limit."),
            Some(PaneStatus::Error)
        );
        assert_eq!(codex.detect_status("$ ls\nREADME.md"), None);
    }

    #[test]
    fn a_rollout_is_read_into_the_shape_otag_uses() {
        let transcript = parse_rollout(&fixture("codex-rollout.jsonl"), None);

        assert_eq!(
            transcript.session_id.as_deref(),
            Some("019fc2ec-4d09-7d31-a2b1-e9f95a59a39b")
        );
        assert_eq!(transcript.cwd, Some(PathBuf::from("/projects/app")));
        assert_eq!(transcript.git_branch.as_deref(), Some("main"));
        assert_eq!(transcript.model.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(transcript.turns, 1);
        assert_eq!(transcript.prompts, vec!["add a test for the parser"]);
        assert_eq!(transcript.last_message.as_deref(), Some("The test passes."));
        assert_eq!(transcript.commands, vec!["cargo test".to_owned()]);
        assert_eq!(transcript.context_window, Some(258_400));
    }

    #[test]
    fn a_rollout_reports_the_tokens_the_agent_counted() {
        let transcript = parse_rollout(&fixture("codex-rollout.jsonl"), None);
        let tokens = transcript.tokens;

        // The agent counts the cached part inside its input tokens; otag
        // keeps them apart so a count means the same for every agent.
        assert_eq!(tokens.input, 41_848 - 5_504);
        assert_eq!(tokens.cache_read, 5_504);
        assert_eq!(tokens.output, 138);
        assert_eq!(tokens.context, 41_848);
    }

    #[test]
    fn a_rollout_carries_the_agent_s_own_rate_limits() {
        // No other source has these: the agent writes them into its session
        // file and nowhere else.
        let transcript = parse_rollout(&fixture("codex-rollout.jsonl"), None);

        let five_hour = transcript.limits.five_hour.expect("the short window");
        assert!((five_hour.used_pct - 36.0).abs() < f32::EPSILON);
        assert_eq!(five_hour.resets_at, Some(1_792_626_399));

        let weekly = transcript.limits.weekly.expect("the long window");
        assert!((weekly.used_pct - 9.0).abs() < f32::EPSILON);
        assert_eq!(weekly.resets_at, Some(1_793_094_492));
    }

    #[test]
    fn the_windows_are_sorted_by_how_long_they_are_not_by_their_order() {
        let text = r#"{"type":"event_msg","payload":{"type":"token_count","rate_limits":{"primary":{"used_percent":9.0,"window_minutes":10080},"secondary":{"used_percent":36.0,"window_minutes":300}}}}"#;
        let limits = parse_rollout(text, None).limits;
        assert!((limits.five_hour.expect("the short window").used_pct - 36.0).abs() < f32::EPSILON);
        assert!((limits.weekly.expect("the long window").used_pct - 9.0).abs() < f32::EPSILON);
    }

    #[test]
    fn a_window_without_a_percentage_is_not_a_window() {
        let text = r#"{"type":"event_msg","payload":{"type":"token_count","rate_limits":{"primary":{"window_minutes":300}}}}"#;
        assert!(parse_rollout(text, None).limits.is_empty());
    }

    #[test]
    fn a_rollout_line_that_cannot_be_read_does_not_hide_the_rest() {
        let text = concat!(
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"first"}]}}"#,
            "\nnot json at all\n",
            r#"{"type":"from-the-future","payload":{"nobody":"knows"}}"#,
            "\n",
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"second"}]}}"#,
            "\n",
            r#"{"type":"response_item","payload":{"type":"mess"#
        );

        let transcript = parse_rollout(text, None);
        assert_eq!(
            transcript.prompts,
            vec!["first".to_owned(), "second".to_owned()]
        );
    }

    #[test]
    fn an_empty_rollout_reads_as_nothing_rather_than_failing() {
        let transcript = parse_rollout("", Some("an-id".to_owned()));
        assert_eq!(transcript.session_id.as_deref(), Some("an-id"));
        assert_eq!(transcript.turns, 0);
        assert_eq!(transcript.tokens, Tokens::default());
    }

    #[test]
    fn a_rollout_file_that_is_not_there_is_an_error_that_names_it() {
        let error = Codex
            .read_transcript(Path::new("/no/such/rollout.jsonl"))
            .expect_err("there is no such file");
        assert!(
            error.to_string().contains("/no/such/rollout.jsonl"),
            "{error}"
        );
        assert!(error.to_string().contains("codex"), "{error}");
    }

    #[test]
    fn a_session_identifier_is_taken_from_the_file_name() {
        assert_eq!(
            session_id_from_path(Path::new(
                "/s/2026/08/02/rollout-2026-08-02T17-41-33-019fc2ec-4d09-7d31-a2b1-e9f95a59a39b.jsonl"
            ))
            .as_deref(),
            Some("019fc2ec-4d09-7d31-a2b1-e9f95a59a39b")
        );
        // A name that does not end in an identifier is not one.
        assert_eq!(session_id_from_path(Path::new("/s/notes.jsonl")), None);
        assert_eq!(
            session_id_from_path(Path::new("/s/rollout-2026-08-02T17-41-33.jsonl")),
            None
        );
    }

    #[test]
    fn a_shell_call_s_command_is_read_however_it_was_given() {
        assert_eq!(
            shell_command(r#"{"command":["bash","-lc","cargo test"]}"#).as_deref(),
            Some("cargo test"),
            "the shell in front of a script says nothing about what happened"
        );
        assert_eq!(
            shell_command(r#"{"command":["git","status","--porcelain"]}"#).as_deref(),
            Some("git status --porcelain")
        );
        assert_eq!(
            shell_command(r#"{"command":"cargo test"}"#).as_deref(),
            Some("cargo test")
        );
        assert_eq!(shell_command("{}"), None);
        assert_eq!(shell_command("not json"), None);
    }

    /// A sessions directory with one rollout in it for the given day.
    fn with_rollout(home: &Path, day: &str, id: &str, body: &str) -> PathBuf {
        let directory = Codex.session_store(home).join(day);
        std::fs::create_dir_all(&directory).expect("the dated directory");
        let path = directory.join(format!("rollout-2026-01-01T00-00-00-{id}.jsonl"));
        std::fs::write(&path, body).expect("the rollout");
        path
    }

    #[test]
    fn the_newest_rollout_for_a_directory_is_the_one_found() {
        let home = tempfile::tempdir().expect("a temporary home");
        let line =
            |cwd: &str| format!(r#"{{"type":"session_meta","payload":{{"cwd":"{cwd}"}}}}"#) + "\n";

        with_rollout(
            home.path(),
            "2026/01/01",
            "019fc2ec-4d09-7d31-a2b1-000000000001",
            &line("/projects/app"),
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
        with_rollout(
            home.path(),
            "2026/01/02",
            "019fc2ec-4d09-7d31-a2b1-000000000002",
            &line("/projects/app"),
        );
        with_rollout(
            home.path(),
            "2026/01/02",
            "019fc2ec-4d09-7d31-a2b1-000000000003",
            &line("/projects/other"),
        );

        let session = Codex
            .find_session_for(home.path(), Path::new("/projects/app"), None)
            .expect("a session is found");
        assert_eq!(session.id, "019fc2ec-4d09-7d31-a2b1-000000000002");
        assert_eq!(session.cwd, Some(PathBuf::from("/projects/app")));
    }

    #[test]
    fn a_rollout_without_a_directory_in_it_is_not_matched_to_one() {
        let home = tempfile::tempdir().expect("a temporary home");
        with_rollout(
            home.path(),
            "2026/01/01",
            "019fc2ec-4d09-7d31-a2b1-000000000001",
            "{\"type\":\"session_meta\",\"payload\":{}}\n",
        );
        assert_eq!(
            Codex.find_session_for(home.path(), Path::new("/projects/app"), None),
            None
        );
    }

    #[test]
    fn a_home_with_no_sessions_yields_nothing() {
        let home = tempfile::tempdir().expect("a temporary home");
        assert_eq!(
            Codex.find_session_for(home.path(), Path::new("/projects/app"), None),
            None
        );
        assert!(rollouts(&Codex.session_store(home.path())).is_empty());
    }

    #[test]
    fn only_the_dated_directories_are_walked() {
        // Whatever else the agent keeps in there is not otag's business.
        let home = tempfile::tempdir().expect("a temporary home");
        let deep = Codex
            .session_store(home.path())
            .join("2026/01/01/extra/deeper");
        std::fs::create_dir_all(&deep).expect("a deeper directory");
        std::fs::write(deep.join("rollout-a-b-c-d-e.jsonl"), "{}\n").expect("a file");

        assert!(rollouts(&Codex.session_store(home.path())).is_empty());
    }
}
