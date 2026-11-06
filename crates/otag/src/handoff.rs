//! Carrying a task from one agent to the next.
//!
//! Two things can be done when an agent runs out of road, and otag offers
//! both:
//!
//! - **Switch agent**: assemble a package from what is on disk and open the
//!   other agent beside the pane, told to read it. Nothing asks the agent
//!   being handed from, because it may be out of quota.
//! - **Switch model**: when the agent itself still has quota and only one
//!   model is exhausted, resume the same session on a lighter model. The
//!   conversation is kept, so there is no package to write.
//!
//! The pane that was working is left exactly as it is. It may come back when
//! its window resets, and otag deciding to close it would throw that away.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use otag_agents::{AgentKind, SessionRef, Transcript};
use otag_core::layout::Direction;
use otag_core::pane::PaneId;
use otag_core::session::Session;
use otag_core::snapshot;
use otag_features::handoff::Package;
use otag_features::usage::now;

use crate::git;

/// The model a handoff resumes on when the user names none.
///
/// Claude Code takes a model alias on the command line, which is verified
/// against its `--help`. Codex's resume flags are not verified on this
/// machine, so otag has no default for it and says so rather than guessing.
const FALLBACK_MODEL: [(AgentKind, Option<&str>); 2] = [
    (AgentKind::Claude, Some("sonnet")),
    (AgentKind::Codex, None),
];

/// How much of the pane's screen is read for error lines.
const SCREEN_LINES: usize = 200;

/// What a handoff is asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// Open another agent on the same task.
    SwitchAgent {
        /// Which agent, or the other one otag knows.
        to: Option<AgentKind>,
    },
    /// Continue the same session on another model of the same agent.
    SwitchModel {
        /// Which model, or the agent's configured fallback.
        model: Option<String>,
    },
}

/// What a handoff did, for the client to report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Done {
    /// The pane the work carries on in.
    pub pane: PaneId,
    /// The package that was written, when one was.
    pub package: Option<PathBuf>,
    /// One line saying what happened, for the user.
    pub summary: String,
}

/// How long after a handoff another from the same pane is refused.
///
/// A handoff takes a moment — a transcript to read, `git` to ask, a package
/// to write — and the button that starts one is sitting there the whole time.
/// Long enough to cover that, short enough that a user who really does want a
/// third agent on the task only has to wait.
const COOLING: Duration = Duration::from_secs(20);

/// The panes a handoff has just been made from.
///
/// A handoff opens a session and writes a file, so asking for one twice is
/// not harmless: the first real report of this was two agents opened on one
/// task from a double click. The second request is refused with a sentence
/// saying so rather than ignored, because a user who meant it deserves to
/// know why nothing happened.
#[derive(Debug, Default)]
pub struct Recent {
    from: std::collections::HashMap<PaneId, Instant>,
}

impl Recent {
    /// Nothing handed over yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Take the right to hand `pane` over, or say why it cannot be taken.
    ///
    /// Held from the moment the handoff starts, so a second request arriving
    /// while the first is still assembling its package is refused too.
    pub fn claim(&mut self, pane: PaneId) -> Result<(), String> {
        self.from.retain(|_, at| at.elapsed() < COOLING);
        if self.from.contains_key(&pane) {
            return Err(format!(
                "otag handed pane {pane} over a moment ago. \
                 Look at the session it opened; if you really want another, \
                 ask again in a few seconds."
            ));
        }
        self.from.insert(pane, Instant::now());
        Ok(())
    }

    /// Give the right back, for a handoff that did not happen.
    ///
    /// A refused handoff leaves nothing behind, so the user can correct what
    /// was wrong with it and ask again straight away.
    pub fn release(&mut self, pane: PaneId) {
        self.from.remove(&pane);
    }
}

/// Hand the task in `pane` over, and report what happened.
pub async fn run(session: &Arc<Session>, pane: PaneId, mode: Mode) -> Result<Done, String> {
    let panes = session.agent_panes();
    let found = panes
        .iter()
        .find(|candidate| candidate.id == pane)
        .ok_or_else(|| format!("otag has no pane {pane}"))?;
    let from = found.agent.ok_or_else(|| {
        format!("pane {pane} is not running an agent otag knows, so there is nothing to hand over")
    })?;

    match mode {
        Mode::SwitchModel { model } => {
            let session_id = found.session_id.clone().ok_or_else(|| {
                format!(
                    "otag does not know which {from} session pane {pane} is running, \
                     so it cannot resume it. Install otag's hooks (`otag install-hooks`) \
                     so that sessions are reported."
                )
            })?;
            let model = model
                .or_else(|| fallback_model(from).map(str::to_owned))
                .ok_or_else(|| {
                    format!(
                        "otag has no verified fallback model for {from}. \
                         Pass the model to use, for example `--model <name>`."
                    )
                })?;

            let reference = SessionRef {
                id: session_id.clone(),
                path: found.session_path.clone().unwrap_or_default(),
                cwd: found.cwd.clone(),
            };
            let command = from.adapter().resume_on_model(&reference, &model);
            let started = split(session, pane, &command, from).await?;

            Ok(Done {
                pane: started,
                package: None,
                summary: format!("{from} continues session {session_id} on {model}"),
            })
        }
        Mode::SwitchAgent { to } => {
            let to = to.unwrap_or_else(|| other(from));
            if to == from {
                return Err(format!(
                    "pane {pane} is already running {from}. \
                     To continue on another model, hand it over with a model instead."
                ));
            }

            let cwd = found.cwd.clone().ok_or_else(|| {
                format!("otag does not know which directory pane {pane} is working in")
            })?;
            let transcript = read_transcript(from, found.session_path.clone()).await;
            let screen = screen_lines(session, pane).await;
            let checkout = git::checkout(&cwd).await;

            let package = Package::new(Some(from), Some(to), &transcript, checkout, &screen)
                .because(format!("{from} could not carry on in this pane"));
            let path = package.path(&cwd, now());
            write(&path, &package.render()).map_err(|error| {
                format!(
                    "otag cannot write the handoff package {}: {error}",
                    path.display()
                )
            })?;

            // The package is otag's, not the user's work: keep it out of
            // their `git status` without touching a tracked file.
            if let Err(error) = git::exclude(&cwd) {
                tracing::debug!(%error, "cannot add otag's directory to .git/info/exclude");
            }

            let command = to.adapter().start_command(
                Some(&Package::prompt(&path)),
                session.agent_model(to).as_deref(),
            );
            let started = split(session, pane, &command, to).await?;

            Ok(Done {
                pane: started,
                package: Some(path.clone()),
                summary: format!(
                    "{to} is taking over from {from}, reading {}",
                    path.display()
                ),
            })
        }
    }
}

/// The agent a handoff goes to when the user names none.
fn other(from: AgentKind) -> AgentKind {
    AgentKind::ALL
        .into_iter()
        .find(|candidate| *candidate != from)
        .unwrap_or(from)
}

/// The model an agent resumes on when the user names none.
fn fallback_model(agent: AgentKind) -> Option<&'static str> {
    FALLBACK_MODEL
        .into_iter()
        .find(|(candidate, _)| *candidate == agent)
        .and_then(|(_, model)| model)
}

/// Open the agent that takes over beside the pane handed over from.
async fn split(
    session: &Arc<Session>,
    beside: PaneId,
    command: &[String],
    agent: AgentKind,
) -> Result<PaneId, String> {
    let label = match agent {
        AgentKind::Claude => "Claude",
        AgentKind::Codex => "Codex",
        AgentKind::Deepseek => "DeepSeek",
    };
    session
        .split_running(beside, Direction::LeftRight, command, label)
        .await
        .map_err(|error| format!("otag cannot open a pane for {agent}: {error}"))
}

/// Read what the agent wrote about its session, as far as it can be read.
///
/// A handoff without a transcript is still worth making: the checkout and the
/// pane's errors say a good deal on their own.
async fn read_transcript(agent: AgentKind, path: Option<PathBuf>) -> Transcript {
    let Some(path) = path else {
        return Transcript::default();
    };
    let read = tokio::task::spawn_blocking(move || agent.adapter().read_transcript(&path)).await;
    match read {
        Ok(Ok(transcript)) => transcript,
        Ok(Err(error)) => {
            tracing::debug!(%error, "a session file could not be read for a handoff");
            Transcript::default()
        }
        Err(error) => {
            tracing::debug!(%error, "reading a session file for a handoff was cancelled");
            Transcript::default()
        }
    }
}

/// The last lines the pane showed, for the errors in the package.
async fn screen_lines(session: &Arc<Session>, pane: PaneId) -> Vec<String> {
    let Some(handle) = session.pane(pane) else {
        return Vec::new();
    };
    match handle
        .history(SCREEN_LINES.min(snapshot::HISTORY_LINES))
        .await
    {
        Ok(lines) => lines,
        Err(error) => {
            tracing::debug!(%error, "cannot read a pane's screen for a handoff");
            Vec::new()
        }
    }
}

/// Write the package, creating the directory it goes in.
fn write(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(directory) = path.parent() {
        std::fs::create_dir_all(directory)?;
    }
    std::fs::write(path, text)
}

#[cfg(test)]
mod tests {
    use otag_core::pane::{PaneSize, PaneSpec};

    use super::*;

    /// A program that stands in for an agent: it stays up and says nothing.
    const FAKE_AGENT: &str = "#!/bin/sh\nsleep 30\n";

    /// Put a fake `claude` and `codex` on a path of their own, so that a
    /// handoff can really open one without either being installed.
    fn fake_agents(dir: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).expect("a directory for the fakes");
        for agent in AgentKind::ALL {
            let path = bin.join(agent.name());
            std::fs::write(&path, FAKE_AGENT).expect("a fake agent");
            let mut permissions = std::fs::metadata(&path)
                .expect("the file exists")
                .permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&path, permissions).expect("it can be run");
        }
        bin
    }

    /// A session with one pane, running something that stays up.
    async fn with_one_pane(cwd: &Path) -> (Arc<Session>, PaneId) {
        let bin = fake_agents(cwd);
        let path = match std::env::var("PATH") {
            Ok(existing) => format!("{}:{existing}", bin.display()),
            Err(_) => bin.display().to_string(),
        };
        let session = Session::new(PaneSpec::command(["sleep", "30"]).with_env("PATH", path));
        session
            .attach(PaneSize::new(24, 80), Some(cwd))
            .await
            .expect("a pane");
        let pane = session
            .visible_panes()
            .first()
            .copied()
            .expect("the pane is there");
        (session, pane)
    }

    #[test]
    fn the_other_agent_is_the_one_that_is_not_this_one() {
        assert_eq!(other(AgentKind::Claude), AgentKind::Codex);
        assert_eq!(other(AgentKind::Codex), AgentKind::Claude);
    }

    #[test]
    fn a_fallback_model_is_only_offered_where_it_was_verified() {
        assert_eq!(fallback_model(AgentKind::Claude), Some("sonnet"));
        assert_eq!(
            fallback_model(AgentKind::Codex),
            None,
            "otag does not guess at flags it has not seen"
        );
    }

    #[tokio::test]
    async fn a_pane_without_an_agent_cannot_be_handed_over() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let (session, pane) = with_one_pane(dir.path()).await;

        let error = run(&session, pane, Mode::SwitchAgent { to: None })
            .await
            .expect_err("there is nothing to hand over");
        assert!(error.contains("not running an agent"), "{error}");
    }

    #[tokio::test]
    async fn a_pane_otag_does_not_have_cannot_be_handed_over() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let (session, pane) = with_one_pane(dir.path()).await;

        let error = run(&session, pane.next().next(), Mode::SwitchAgent { to: None })
            .await
            .expect_err("no such pane");
        assert!(error.contains("no pane"), "{error}");
    }

    #[tokio::test]
    async fn handing_over_writes_a_package_and_opens_the_other_agent() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let (session, pane) = with_one_pane(dir.path()).await;
        session.set_agent(pane, Some(AgentKind::Claude)).await;

        let done = run(&session, pane, Mode::SwitchAgent { to: None })
            .await
            .expect("the handoff happens");

        let path = done.package.expect("a package was written");
        let written = std::fs::read_to_string(&path).expect("the package");
        assert!(
            written.starts_with("# Handoff: claude → codex"),
            "{written}"
        );
        assert!(
            path.starts_with(dir.path().join(".otag").join("handoff")),
            "{}",
            path.display()
        );
        assert!(
            done.summary.contains("codex is taking over"),
            "{}",
            done.summary
        );

        assert_eq!(session.visible_panes().len(), 2, "the old pane stays open");
        assert!(session.visible_panes().contains(&pane));
        assert!(session.visible_panes().contains(&done.pane));
    }

    #[tokio::test]
    async fn handoff_names_and_nicknames_follow_the_source_in_both_directions() {
        for from in AgentKind::ALL {
            let dir = tempfile::tempdir().expect("a temporary directory");
            let (session, pane) = with_one_pane(dir.path()).await;
            session.set_agent(pane, Some(from)).await;
            session.set_first_prompt(pane, "Fix parser tests").await;
            session.set_nickname(pane, "backend").await;

            let done = run(&session, pane, Mode::SwitchAgent { to: None })
                .await
                .expect("the handoff happens");
            let path = done.package.expect("a package was written");
            // A later transcript read must never replace the seeded name
            // with otag's own first instruction.
            session
                .set_first_prompt(done.pane, &Package::prompt(&path))
                .await;
            let panes = session.summaries();
            let started = panes.iter().find(|p| p.id == done.pane).expect("new pane");
            let expected = match from {
                AgentKind::Claude => "Fix parser tests (Codex)",
                AgentKind::Codex => "Fix parser tests (Claude)",
                AgentKind::Deepseek => "Fix parser tests (Claude)",
            };
            assert_eq!(started.first_prompt.as_deref(), Some(expected));
            assert_eq!(started.nickname, "backend");
            let source = panes.iter().find(|p| p.id == pane).expect("source pane");
            assert_eq!(source.first_prompt.as_deref(), Some("Fix parser tests"));
            assert_eq!(source.nickname, "backend");
        }
    }

    #[tokio::test]
    async fn handing_over_to_the_same_agent_is_refused_with_a_reason() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let (session, pane) = with_one_pane(dir.path()).await;
        session.set_agent(pane, Some(AgentKind::Claude)).await;

        let error = run(
            &session,
            pane,
            Mode::SwitchAgent {
                to: Some(AgentKind::Claude),
            },
        )
        .await
        .expect_err("there is nothing to switch to");
        assert!(error.contains("already running claude"), "{error}");
    }

    #[tokio::test]
    async fn continuing_on_another_model_needs_a_session_to_continue() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let (session, pane) = with_one_pane(dir.path()).await;
        session.set_agent(pane, Some(AgentKind::Claude)).await;

        let error = run(&session, pane, Mode::SwitchModel { model: None })
            .await
            .expect_err("no session is known");
        assert!(error.contains("install-hooks"), "{error}");
    }

    #[tokio::test]
    async fn continuing_on_another_model_resumes_the_same_session() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let (session, pane) = with_one_pane(dir.path()).await;
        session.set_agent(pane, Some(AgentKind::Claude)).await;
        session.set_session_id(pane, "abc-123".to_owned()).await;

        let done = run(&session, pane, Mode::SwitchModel { model: None })
            .await
            .expect("the session is resumed");

        assert!(
            done.package.is_none(),
            "the conversation is kept, so no package"
        );
        assert!(done.summary.contains("abc-123"), "{}", done.summary);
        assert!(done.summary.contains("sonnet"), "{}", done.summary);

        let resumed = session
            .snapshot()
            .panes
            .into_iter()
            .find(|record| record.id == done.pane)
            .expect("the new pane is in the snapshot");
        assert_eq!(
            resumed.command,
            vec!["claude", "--resume", "abc-123", "--model", "sonnet"],
            "the same session, on the other model"
        );
    }

    #[tokio::test]
    async fn a_model_the_user_names_is_used_as_it_is() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let (session, pane) = with_one_pane(dir.path()).await;
        session.set_agent(pane, Some(AgentKind::Claude)).await;
        session.set_session_id(pane, "abc".to_owned()).await;

        let done = run(
            &session,
            pane,
            Mode::SwitchModel {
                model: Some("haiku".to_owned()),
            },
        )
        .await
        .expect("the session is resumed");
        assert!(done.summary.contains("haiku"), "{}", done.summary);
    }

    #[tokio::test]
    async fn an_agent_with_no_verified_fallback_model_says_so() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let (session, pane) = with_one_pane(dir.path()).await;
        session.set_agent(pane, Some(AgentKind::Codex)).await;
        session.set_session_id(pane, "abc".to_owned()).await;

        let error = run(&session, pane, Mode::SwitchModel { model: None })
            .await
            .expect_err("otag has no model to offer");
        assert!(error.contains("--model"), "{error}");
    }

    #[test]
    fn a_handoff_asked_for_twice_is_only_made_once() {
        let mut recent = Recent::new();
        let pane = PaneId::FIRST;

        assert!(recent.claim(pane).is_ok(), "the first one goes ahead");
        let refused = recent.claim(pane).expect_err("the second one does not");
        assert!(refused.contains("a moment ago"), "{refused}");

        // Another session is another task, and is not held up by this one.
        assert!(recent.claim(pane.next()).is_ok());
    }

    #[test]
    fn a_handoff_that_did_not_happen_can_be_asked_for_again_at_once() {
        let mut recent = Recent::new();
        let pane = PaneId::FIRST;

        assert!(recent.claim(pane).is_ok());
        // Nothing was opened and nothing written, so the user can correct
        // what was wrong with it and try again.
        recent.release(pane);
        assert!(recent.claim(pane).is_ok());
    }
}
