//! What an agent's hooks tell otag.
//!
//! Claude Code runs a command on each of a handful of events and writes a
//! JSON object to its standard input. otag registers `otag hook <event>` for
//! the events it cares about, and this module turns what arrives into
//! something the daemon can act on.
//!
//! Hooks are the primary source of a pane's status: they say what the agent
//! is doing rather than what its screen happens to show. Every field is
//! optional, because the payload differs per event and grows per release.

use std::path::PathBuf;

use serde::Deserialize;

use crate::PaneStatus;

/// The events otag asks to be told about.
///
/// Each costs the agent one process per occurrence, so the list is only as
/// long as it has to be:
///
/// - `SessionStart` carries the session identifier otag resumes by.
/// - `Notification` is the agent asking for the user.
/// - `PreToolUse` says which tool is waiting, for the approval inbox.
/// - `PostToolUse` says which file was written, for the conflict radar.
/// - `Stop` and `SubagentStop` say the agent has finished answering.
pub const EVENTS: [&str; 6] = [
    "SessionStart",
    "Notification",
    "PreToolUse",
    "PostToolUse",
    "Stop",
    "SubagentStop",
];

/// The name otag registers for the statusLine command, which is not a hook
/// but arrives the same way.
pub const STATUS_LINE: &str = "statusline";

/// Tools whose use means a file was written to.
const WRITING_TOOLS: [&str; 4] = ["Edit", "Write", "MultiEdit", "NotebookEdit"];

/// What an agent sends a hook command on its standard input.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct HookPayload {
    /// Which event this is. otag trusts the event it registered over this,
    /// because an agent may rename an event before otag knows about it.
    #[serde(default)]
    pub hook_event_name: Option<String>,
    /// The session the event belongs to.
    #[serde(default)]
    pub session_id: Option<String>,
    /// The file the session is kept in.
    #[serde(default)]
    pub transcript_path: Option<PathBuf>,
    /// Where the agent is working.
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    /// Which tool is about to run, or has just run.
    #[serde(default)]
    pub tool_name: Option<String>,
    /// What the tool was given.
    #[serde(default)]
    pub tool_input: Option<ToolInput>,
    /// What the agent wants to tell the user.
    #[serde(default)]
    pub message: Option<String>,
    /// Why a session started: new, resumed, or cleared.
    #[serde(default)]
    pub source: Option<String>,
}

/// The part of a tool's input otag reads.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ToolInput {
    /// The file a tool is about to write, or has written.
    #[serde(default)]
    pub file_path: Option<PathBuf>,
    /// The command a tool is about to run.
    #[serde(default)]
    pub command: Option<String>,
    /// What the tool is for, in the agent's own words.
    #[serde(default)]
    pub description: Option<String>,
}

impl HookPayload {
    /// Read what the agent sent, or nothing when it is not JSON at all.
    ///
    /// A hook that cannot read its payload still has to exit cleanly and
    /// quickly: blocking the agent would be worse than losing one event.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        serde_json::from_str(text).ok()
    }

    /// What this event says the pane's status is, if it says anything.
    ///
    /// `event` is the name otag registered the hook under, which is more
    /// trustworthy than the one in the payload.
    #[must_use]
    pub fn status(&self, event: &str) -> Option<PaneStatus> {
        match event {
            "Notification" => Some(PaneStatus::NeedsApproval),
            "PreToolUse" => Some(PaneStatus::NeedsApproval),
            "PostToolUse" => Some(PaneStatus::Working),
            "Stop" | "SubagentStop" => Some(PaneStatus::Done),
            "SessionStart" => Some(PaneStatus::Idle),
            _ => None,
        }
    }

    /// The file this event says was written to, for the conflict radar.
    #[must_use]
    pub fn written_file(&self, event: &str) -> Option<&std::path::Path> {
        if event != "PostToolUse" {
            return None;
        }
        let tool = self.tool_name.as_deref()?;
        if !WRITING_TOOLS.contains(&tool) {
            return None;
        }
        self.tool_input.as_ref()?.file_path.as_deref()
    }

    /// A line describing what is waiting, for the approval inbox.
    #[must_use]
    pub fn summary(&self) -> Option<String> {
        if let Some(message) = self.message.as_deref().filter(|text| !text.is_empty()) {
            return Some(message.to_owned());
        }

        let tool = self.tool_name.as_deref()?;
        let input = self.tool_input.as_ref();
        let detail = input
            .and_then(|input| input.command.as_deref())
            .or_else(|| input.and_then(|input| input.description.as_deref()))
            .or_else(|| {
                input
                    .and_then(|input| input.file_path.as_deref())
                    .and_then(std::path::Path::to_str)
            });

        Some(match detail {
            Some(detail) => format!("{tool}: {detail}"),
            None => tool.to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn fixture(name: &str) -> String {
        let path = Path::new(crate::FIXTURES).join(name);
        std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("the fixture {} is readable: {error}", path.display()))
    }

    #[test]
    fn a_session_starting_carries_the_identifier_otag_resumes_by() {
        let payload =
            HookPayload::parse(&fixture("claude-hook-session-start.json")).expect("it parses");

        assert_eq!(
            payload.session_id.as_deref(),
            Some("11111111-2222-3333-4444-555555555555")
        );
        assert_eq!(payload.cwd, Some(PathBuf::from("/projects/app")));
        assert_eq!(payload.source.as_deref(), Some("startup"));
        assert_eq!(payload.status("SessionStart"), Some(PaneStatus::Idle));
    }

    #[test]
    fn a_tool_waiting_for_permission_is_reported_with_what_it_would_run() {
        let payload =
            HookPayload::parse(&fixture("claude-hook-pre-tool-use.json")).expect("it parses");

        assert_eq!(payload.tool_name.as_deref(), Some("Bash"));
        assert_eq!(
            payload.status("PreToolUse"),
            Some(PaneStatus::NeedsApproval)
        );
        assert_eq!(payload.summary().as_deref(), Some("Bash: cargo test"));
    }

    #[test]
    fn a_tool_that_has_written_a_file_is_reported_with_its_path() {
        let payload =
            HookPayload::parse(&fixture("claude-hook-post-tool-use.json")).expect("it parses");

        assert_eq!(
            payload.written_file("PostToolUse"),
            Some(Path::new("/projects/app/src/parser.rs"))
        );
        assert_eq!(payload.status("PostToolUse"), Some(PaneStatus::Working));
    }

    #[test]
    fn only_a_tool_that_writes_counts_as_having_written() {
        let payload =
            HookPayload::parse(r#"{"tool_name":"Read","tool_input":{"file_path":"/a/b.rs"}}"#)
                .expect("it parses");
        assert_eq!(payload.written_file("PostToolUse"), None);
        // And only the event that says a tool has run.
        let payload =
            HookPayload::parse(r#"{"tool_name":"Edit","tool_input":{"file_path":"/a/b.rs"}}"#)
                .expect("it parses");
        assert_eq!(payload.written_file("PreToolUse"), None);
    }

    #[test]
    fn a_notification_is_the_agent_asking_for_the_user() {
        let payload =
            HookPayload::parse(&fixture("claude-hook-notification.json")).expect("it parses");

        assert_eq!(
            payload.status("Notification"),
            Some(PaneStatus::NeedsApproval)
        );
        assert_eq!(
            payload.summary().as_deref(),
            Some("Claude needs your permission to use Bash")
        );
    }

    #[test]
    fn the_agent_finishing_is_a_status_of_its_own() {
        let payload = HookPayload::parse(&fixture("claude-hook-stop.json")).expect("it parses");
        assert_eq!(payload.status("Stop"), Some(PaneStatus::Done));
        assert_eq!(payload.status("SubagentStop"), Some(PaneStatus::Done));
    }

    #[test]
    fn an_event_otag_does_not_know_says_nothing_about_the_status() {
        let payload = HookPayload::default();
        assert_eq!(payload.status("FromTheFuture"), None);
        assert_eq!(payload.summary(), None);
    }

    #[test]
    fn a_payload_that_is_not_json_is_not_read_as_one() {
        assert!(HookPayload::parse("").is_none());
        assert!(HookPayload::parse("not json at all").is_none());
        // Anything unknown in it is ignored rather than refused: the payload
        // differs per event and grows per release.
        let payload =
            HookPayload::parse(r#"{"from_the_future":{"nobody":"knows"}}"#).expect("it parses");
        assert_eq!(payload.session_id, None);
    }

    #[test]
    fn the_events_otag_registers_are_the_ones_it_acts_on() {
        for event in EVENTS {
            assert!(
                HookPayload::default().status(event).is_some(),
                "{event} is registered but says nothing about a pane"
            );
        }
    }
}
