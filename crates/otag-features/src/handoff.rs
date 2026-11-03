//! Handing a task from one agent to another.
//!
//! The point of a handoff is that it works when the agent being handed from
//! cannot answer any more: out of quota, or stuck. So nothing here asks an
//! agent anything. The package is assembled from what is already on disk —
//! the transcript the agent wrote, what `git` says about the checkout, and
//! the last error-looking lines on the pane — and rendered into one Markdown
//! file the next agent is told to read.
//!
//! Everything is capped. A file that tries to carry a whole session would
//! cost the receiving agent its context before it has done any work, so each
//! section keeps the newest few entries and says when it dropped the rest.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use otag_agents::{AgentKind, Transcript};
use serde::{Deserialize, Serialize};

/// Directory, below the project, that otag writes handoff packages into.
pub const DIRECTORY: [&str; 2] = [".otag", "handoff"];

/// The line added to a project's `.git/info/exclude`.
///
/// `info/exclude` rather than `.gitignore`: the latter is a tracked file, and
/// otag has no business committing to a user's repository.
pub const EXCLUDE_LINE: &str = "/.otag/";

/// How many of the user's prompts are carried.
const PROMPTS: usize = 5;

/// How many files the agent touched are carried.
const FILES: usize = 20;

/// How many shell commands are carried.
const COMMANDS: usize = 10;

/// How many error-looking lines are carried.
const ERRORS: usize = 20;

/// How many lines of `git status` or `git diff --stat` are carried.
const GIT_LINES: usize = 40;

/// How much of one prompt or message is carried, in characters.
const TEXT: usize = 1_500;

/// Words that make a line worth carrying as an error.
const ERROR_MARKERS: [&str; 10] = [
    "error",
    "Error",
    "ERROR",
    "panic",
    "failed",
    "Failed",
    "FAILED",
    "Traceback",
    "exception",
    "Exception",
];

/// What `git` says about the checkout a pane is working in.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Checkout {
    /// The branch that is checked out.
    #[serde(default)]
    pub branch: Option<String>,
    /// `git status --porcelain`, line by line.
    #[serde(default)]
    pub status: Vec<String>,
    /// `git diff --stat`, line by line.
    #[serde(default)]
    pub diff_stat: Vec<String>,
}

impl Checkout {
    /// Whether git said nothing at all, which is the case outside a
    /// repository.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.branch.is_none() && self.status.is_empty() && self.diff_stat.is_empty()
    }
}

/// Everything that goes into a handoff.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Package {
    /// The agent that was working.
    pub from: Option<AgentKind>,
    /// The agent that is to carry on.
    pub to: Option<AgentKind>,
    /// Where the work is happening.
    pub cwd: Option<PathBuf>,
    /// Why the handoff happened, in one line.
    pub reason: Option<String>,
    /// The session the work was in, so a human can find it again.
    pub session_id: Option<String>,
    /// The model that was answering.
    pub model: Option<String>,
    /// What the user asked, oldest first.
    pub prompts: Vec<String>,
    /// The last thing the agent said.
    pub last_message: Option<String>,
    /// Files the agent wrote to.
    pub files_touched: Vec<PathBuf>,
    /// Shell commands it ran.
    pub commands: Vec<String>,
    /// The last error-looking lines from the pane.
    pub errors: Vec<String>,
    /// What git says about the checkout.
    pub checkout: Checkout,
}

impl Package {
    /// Build a package from what otag already knows.
    #[must_use]
    pub fn new(
        from: Option<AgentKind>,
        to: Option<AgentKind>,
        transcript: &Transcript,
        checkout: Checkout,
        screen: &[String],
    ) -> Self {
        Self {
            from,
            to,
            cwd: transcript.cwd.clone(),
            reason: None,
            session_id: transcript.session_id.clone(),
            model: transcript.model.clone(),
            prompts: last(&transcript.prompts, PROMPTS),
            last_message: transcript.last_message.clone(),
            files_touched: last(&transcript.files_touched, FILES),
            commands: last(&transcript.commands, COMMANDS),
            errors: error_lines(screen, ERRORS),
            checkout,
        }
    }

    /// Say why the work is being handed over.
    #[must_use]
    pub fn because(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }

    /// The package as the Markdown file the next agent reads.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        let from = name(self.from);
        let to = name(self.to);

        let _ = writeln!(out, "# Handoff: {from} → {to}");
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "You are taking over a task another agent was working on. \
             Everything below was collected from files on this machine, not \
             from the other agent: it could not be asked."
        );
        let _ = writeln!(out);

        let _ = writeln!(out, "## Where");
        if let Some(cwd) = &self.cwd {
            let _ = writeln!(out, "- Directory: `{}`", cwd.display());
        }
        if let Some(branch) = &self.checkout.branch {
            let _ = writeln!(out, "- Branch: `{branch}`");
        }
        if let Some(model) = &self.model {
            let _ = writeln!(out, "- {from} was using: `{model}`");
        }
        if let Some(session) = &self.session_id {
            let _ = writeln!(out, "- {from} session: `{session}`");
        }
        if let Some(reason) = &self.reason {
            let _ = writeln!(out, "- Why you are taking over: {reason}");
        }
        let _ = writeln!(out);

        section(&mut out, "What the user asked", &self.prompts, |prompt| {
            format!("- {}", clip(prompt, TEXT))
        });

        if let Some(message) = self.last_message.as_deref().filter(|text| !text.is_empty()) {
            let _ = writeln!(out, "## Where {from} left off");
            let _ = writeln!(out);
            let _ = writeln!(out, "{}", clip(message, TEXT));
            let _ = writeln!(out);
        }

        section(&mut out, "Files it changed", &self.files_touched, |path| {
            format!("- `{}`", path.display())
        });
        section(&mut out, "Commands it ran", &self.commands, |command| {
            format!("- `{}`", clip(command, 200))
        });
        section(&mut out, "Last errors on screen", &self.errors, |line| {
            format!("- `{}`", clip(line, 200))
        });

        if !self.checkout.status.is_empty() || !self.checkout.diff_stat.is_empty() {
            let _ = writeln!(out, "## The checkout");
            let _ = writeln!(out);
            if !self.checkout.status.is_empty() {
                let _ = writeln!(out, "`git status --porcelain`:");
                let _ = writeln!(out);
                let _ = writeln!(out, "```");
                for line in last(&self.checkout.status, GIT_LINES) {
                    let _ = writeln!(out, "{line}");
                }
                let _ = writeln!(out, "```");
                let _ = writeln!(out);
            }
            if !self.checkout.diff_stat.is_empty() {
                let _ = writeln!(out, "`git diff --stat`:");
                let _ = writeln!(out);
                let _ = writeln!(out, "```");
                for line in last(&self.checkout.diff_stat, GIT_LINES) {
                    let _ = writeln!(out, "{line}");
                }
                let _ = writeln!(out, "```");
                let _ = writeln!(out);
            }
        }

        let _ = writeln!(out, "## What to do");
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "Continue the task from where it stopped. Read the files above \
             before changing them, do not undo work that is already done, and \
             say what you are doing differently if you disagree with the \
             approach so far."
        );
        out
    }

    /// What the receiving agent is told, as its first prompt.
    #[must_use]
    pub fn prompt(path: &Path) -> String {
        format!(
            "Read {} and continue the task from where it stopped.",
            path.display()
        )
    }

    /// Where the package goes, below the directory being worked in.
    ///
    /// `at` is a timestamp in seconds since the epoch, so the files of one
    /// day sort in the order they were written.
    #[must_use]
    pub fn path(&self, cwd: &Path, at: i64) -> PathBuf {
        let directory = DIRECTORY
            .iter()
            .fold(cwd.to_path_buf(), |path, part| path.join(part));
        directory.join(format!("{at}-{}-to-{}.md", name(self.from), name(self.to)))
    }
}

/// The lines of a pane worth carrying as errors, newest last.
///
/// Repeats are dropped: an agent that failed the same way twenty times has
/// one error, not twenty, and the receiving agent needs the space.
#[must_use]
pub fn error_lines(screen: &[String], max: usize) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for line in screen {
        let trimmed = line.trim();
        if trimmed.is_empty() || !ERROR_MARKERS.iter().any(|marker| trimmed.contains(marker)) {
            continue;
        }
        if found.iter().any(|kept| kept == trimmed) {
            continue;
        }
        found.push(trimmed.to_owned());
    }
    last(&found, max)
}

/// Write one list as a section, when it has anything in it.
fn section<T>(out: &mut String, title: &str, items: &[T], line: impl Fn(&T) -> String) {
    if items.is_empty() {
        return;
    }
    let _ = writeln!(out, "## {title}");
    let _ = writeln!(out);
    for item in items {
        let _ = writeln!(out, "{}", line(item));
    }
    let _ = writeln!(out);
}

/// The last `max` of a list, in the order they were in.
fn last<T: Clone>(items: &[T], max: usize) -> Vec<T> {
    let start = items.len().saturating_sub(max);
    items[start..].to_vec()
}

/// A piece of text cut to `max` characters, on a character boundary.
fn clip(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.replace('\n', " ");
    }
    let kept: String = text.chars().take(max).collect();
    format!("{} […]", kept.replace('\n', " ").trim_end())
}

/// What an agent is called in a handoff, when otag knows which it was.
fn name(agent: Option<AgentKind>) -> String {
    agent.map_or_else(|| "agent".to_owned(), |agent| agent.name().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transcript() -> Transcript {
        Transcript {
            session_id: Some("abc-123".to_owned()),
            cwd: Some(PathBuf::from("/work/project")),
            git_branch: Some("feature".to_owned()),
            model: Some("a-model".to_owned()),
            prompts: vec![
                "first thing".to_owned(),
                "second thing".to_owned(),
                "third thing".to_owned(),
            ],
            last_message: Some("I was about to run the tests.".to_owned()),
            files_touched: vec![PathBuf::from("src/lib.rs"), PathBuf::from("src/main.rs")],
            commands: vec!["cargo test".to_owned()],
            turns: 3,
            ..Transcript::default()
        }
    }

    fn checkout() -> Checkout {
        Checkout {
            branch: Some("feature".to_owned()),
            status: vec![" M src/lib.rs".to_owned()],
            diff_stat: vec![" src/lib.rs | 4 ++--".to_owned()],
        }
    }

    fn package() -> Package {
        Package::new(
            Some(AgentKind::Claude),
            Some(AgentKind::Codex),
            &transcript(),
            checkout(),
            &["error[E0061]: too many arguments".to_owned()],
        )
    }

    #[test]
    fn a_package_carries_what_the_next_agent_needs() {
        let rendered = package()
            .because("claude reached its five-hour limit")
            .render();

        assert!(
            rendered.starts_with("# Handoff: claude → codex"),
            "{rendered}"
        );
        for expected in [
            "/work/project",
            "`feature`",
            "`a-model`",
            "`abc-123`",
            "claude reached its five-hour limit",
            "third thing",
            "I was about to run the tests.",
            "src/main.rs",
            "cargo test",
            "error[E0061]: too many arguments",
            "git status --porcelain",
            "git diff --stat",
            "Continue the task from where it stopped",
        ] {
            assert!(
                rendered.contains(expected),
                "{expected:?} missing:\n{rendered}"
            );
        }
    }

    #[test]
    fn a_package_says_it_was_assembled_without_asking_the_agent() {
        // The receiving agent must not try to coordinate with an agent that
        // may be out of quota.
        assert!(
            package().render().contains("could not be asked"),
            "the next agent is told where this came from"
        );
    }

    #[test]
    fn an_empty_transcript_still_renders_something_usable() {
        let package = Package::new(None, None, &Transcript::default(), Checkout::default(), &[]);
        let rendered = package.render();

        assert!(rendered.starts_with("# Handoff: agent → agent"));
        assert!(rendered.contains("## What to do"));
        assert!(
            !rendered.contains("## Files it changed"),
            "no empty sections"
        );
        assert!(!rendered.contains("## The checkout"));
    }

    #[test]
    fn only_the_newest_entries_are_carried() {
        let mut long = transcript();
        long.prompts = (0..50).map(|number| format!("prompt {number}")).collect();
        long.files_touched = (0..50)
            .map(|number| PathBuf::from(format!("f{number}")))
            .collect();
        long.commands = (0..50).map(|number| format!("command {number}")).collect();

        let package = Package::new(None, None, &long, Checkout::default(), &[]);
        assert_eq!(package.prompts.len(), PROMPTS);
        assert_eq!(package.prompts[0], "prompt 45", "the newest ones");
        assert_eq!(package.files_touched.len(), FILES);
        assert_eq!(package.commands.len(), COMMANDS);
    }

    #[test]
    fn a_very_long_prompt_is_cut_rather_than_carried_whole() {
        let mut long = transcript();
        long.prompts = vec!["x".repeat(TEXT * 3)];

        let rendered = Package::new(None, None, &long, Checkout::default(), &[]).render();
        assert!(rendered.contains("[…]"), "the cut is visible");
        assert!(
            rendered.len() < TEXT * 3,
            "a handoff must not cost the next agent its context"
        );
    }

    #[test]
    fn a_prompt_with_newlines_stays_on_one_line_in_a_list() {
        let mut multiline = transcript();
        multiline.prompts = vec!["first line\nsecond line".to_owned()];

        let rendered = Package::new(None, None, &multiline, Checkout::default(), &[]).render();
        assert!(rendered.contains("- first line second line"), "{rendered}");
    }

    #[test]
    fn the_error_lines_of_a_pane_are_found_and_deduplicated() {
        let screen = [
            "   Compiling otag".to_owned(),
            "error[E0308]: mismatched types".to_owned(),
            "error[E0308]: mismatched types".to_owned(),
            "    let x = 1;".to_owned(),
            "thread 'main' panicked at src/lib.rs:1".to_owned(),
            String::new(),
        ];

        let found = error_lines(&screen, ERRORS);
        assert_eq!(
            found,
            vec![
                "error[E0308]: mismatched types".to_owned(),
                "thread 'main' panicked at src/lib.rs:1".to_owned(),
            ]
        );
    }

    #[test]
    fn only_the_last_error_lines_are_kept() {
        let screen: Vec<String> = (0..50).map(|number| format!("error {number}")).collect();
        let found = error_lines(&screen, 3);
        assert_eq!(found, vec!["error 47", "error 48", "error 49"]);
    }

    #[test]
    fn a_pane_with_no_errors_carries_none() {
        assert!(error_lines(&["all good".to_owned()], ERRORS).is_empty());
        assert!(error_lines(&[], ERRORS).is_empty());
    }

    #[test]
    fn the_file_is_named_after_the_two_agents_and_the_moment() {
        let path = package().path(Path::new("/work/project"), 1_700_000_000);
        assert_eq!(
            path,
            Path::new("/work/project/.otag/handoff/1700000000-claude-to-codex.md")
        );
    }

    #[test]
    fn the_receiving_agent_is_told_to_read_the_file() {
        let prompt = Package::prompt(Path::new("/work/project/.otag/handoff/x.md"));
        assert_eq!(
            prompt,
            "Read /work/project/.otag/handoff/x.md and continue the task from where it stopped."
        );
    }

    #[test]
    fn a_checkout_knows_when_git_said_nothing() {
        assert!(Checkout::default().is_empty());
        assert!(!checkout().is_empty());
    }
}
