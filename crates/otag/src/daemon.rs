//! The daemon: owns the socket, the panes, and the programs running on them.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use otag_agents::{AgentKind, SessionRef, claude, hooks};
use otag_core::attach;
use otag_core::pane::{PaneId, PaneSize, PaneSpec};
use otag_core::paths::{Dirs, SOCKET_ENV};
use otag_core::proto::{PaneInfo, PaneUsage, Request, Response};
use otag_core::server::{Handler, Outbox, Server};
use otag_core::session::Session;

use crate::{agents, persist, usage};

/// Run the daemon in the foreground until it is asked to stop.
pub fn run(socket: &Path, log_filter: Option<&str>) -> Result<()> {
    // Keep the guard alive for the whole run so buffered log lines are
    // flushed before the process exits.
    let _log_guard = match Dirs::from_env().log_dir() {
        Ok(log_dir) => Some(crate::logging::init_daemon(&log_dir, log_filter)?),
        Err(error) => {
            // Without a log directory the daemon still runs; it just logs to
            // stderr, which `otag` redirects to a file when it spawns us.
            crate::logging::init_client(log_filter.or(Some("info")));
            tracing::warn!(%error, "logging to stderr instead of a file");
            None
        }
    };

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("cannot start the async runtime")?;

    runtime.block_on(serve(socket))
}

async fn serve(socket: &Path) -> Result<()> {
    let server = Server::bind(socket)
        .await
        .with_context(|| format!("cannot start the otag daemon on {}", socket.display()))?;

    tracing::info!(version = otag_core::VERSION, "otag daemon started");
    // Every pane runs the user's shell, told how to reach otag so that agent
    // hooks inside it can report back.
    let session = Session::new(PaneSpec::shell().with_env(SOCKET_ENV, socket));

    // Persistence is a convenience, not a requirement: a daemon that cannot
    // save its state still runs the user's panes.
    let snapshots = open_snapshots();
    let saving = match &snapshots {
        Some(snapshots) => {
            persist::restore(&session, snapshots).await;
            Some(tokio::spawn(persist::keep_saving(
                Arc::clone(&session),
                snapshots.clone(),
            )))
        }
        None => None,
    };

    // The usage history outlives one daemon run: a projection needs the last
    // twenty minutes of samples, which a restart would otherwise throw away.
    let shared_usage = usage::Shared::new();
    if let Some(snapshots) = &snapshots {
        persist::restore_usage(&shared_usage, snapshots).await;
    }
    usage::publish(&session, &shared_usage).await;

    // Agents are noticed whether or not the user installed otag's hooks, so
    // this runs for every daemon.
    let watching = tokio::spawn(agents::keep_watching(
        Arc::clone(&session),
        Arc::clone(&shared_usage),
    ));

    let daemon = Daemon {
        session: Arc::clone(&session),
        usage: Arc::clone(&shared_usage),
    };
    let result = server.run_until(daemon, terminate_signal()).await;
    watching.abort();

    // The panes are about to be killed with the daemon, so what they show
    // now is what a restart has to put back.
    if let Some(snapshots) = &snapshots {
        if let Some(saving) = saving {
            saving.abort();
        }
        persist::save_everything(&session, snapshots).await;
        persist::save_usage(&shared_usage, snapshots).await;
    }

    tracing::info!("otag daemon stopped");
    result.context("the otag daemon stopped with an error")
}

/// Open the snapshot database, or report why the daemon will not remember
/// anything.
fn open_snapshots() -> Option<persist::Snapshots> {
    let state_dir = match Dirs::from_env().state_dir() {
        Ok(state_dir) => state_dir,
        Err(error) => {
            tracing::warn!(%error, "otag will not remember this session");
            return None;
        }
    };

    match persist::Snapshots::open(state_dir.join(otag_persist::DATABASE_FILE_NAME)) {
        Ok(snapshots) => {
            tracing::info!(database = %snapshots.path().display(), "otag is saving its state");
            Some(snapshots)
        }
        Err(error) => {
            tracing::warn!(%error, "otag will not remember this session");
            None
        }
    }
}

/// Everything one daemon owns.
struct Daemon {
    session: Arc<Session>,
    usage: Arc<usage::Shared>,
}

impl Handler for Daemon {
    /// Answer a client's request.
    ///
    /// `Shutdown` and `Detach` never reach here: the server handles its own
    /// lifecycle. Nothing in here may fail in a way that stops the daemon:
    /// a request that cannot be carried out is logged and answered, and the
    /// session keeps running.
    async fn handle(&self, request: Request, outbox: &Outbox) -> Option<Response> {
        match request {
            Request::Ping => Some(Response::Pong),
            Request::Version => Some(Response::Version {
                version: otag_core::VERSION.to_owned(),
            }),
            Request::ListPanes => Some(Response::Panes {
                panes: self
                    .session
                    .summaries()
                    .into_iter()
                    .map(|pane| PaneInfo {
                        id: pane.id,
                        size: pane.size,
                        workspace: pane.workspace,
                        title: pane.title,
                        agent: pane.agent,
                        status: pane.status,
                        session: pane.session,
                        tokens: pane.tokens,
                        exit: pane.exit,
                    })
                    .collect(),
            }),
            Request::Usage => Some(Response::Usage {
                report: Box::new(self.usage.report()),
                panes: self
                    .session
                    .pane_usage()
                    .into_iter()
                    .map(|(pane, agent, tokens)| PaneUsage {
                        pane,
                        agent,
                        tokens,
                    })
                    .collect(),
            }),
            Request::Attach { size, cwd } => Some(self.attach(outbox, size, cwd.as_deref()).await),
            Request::Input { bytes } => {
                if let Err(error) = self.session.input(bytes).await {
                    // A key pressed as a program exits has nowhere to go.
                    // The client learns the pane ended from its own stream.
                    tracing::debug!(%error, "cannot deliver input to a pane");
                }
                None
            }
            Request::Resize { size } => {
                self.session.resize(size).await;
                None
            }
            Request::Split { direction } => {
                self.report("split a pane", self.session.split(direction).await)
            }
            Request::NewTab => self.report("add a tab", self.session.new_tab().await),
            Request::SelectTab { index } => {
                self.session.select_tab(index).await;
                None
            }
            Request::NewWorkspace { cwd } => self.report(
                "add a workspace",
                self.session.new_workspace(cwd.as_deref()).await,
            ),
            Request::SelectWorkspace { index } => {
                self.session.select_workspace(index).await;
                None
            }
            Request::FocusPane { pane } => {
                self.session.focus_pane(pane).await;
                None
            }
            Request::FocusSide { side } => {
                self.session.focus_side(side).await;
                None
            }
            Request::FocusNext => {
                self.session.focus_next().await;
                None
            }
            Request::ResizePane { side, cells } => {
                self.session.resize_pane(side, cells).await;
                None
            }
            Request::ToggleSidebar => {
                self.session.toggle_sidebar().await;
                None
            }
            Request::Hook {
                event,
                pane,
                payload,
            } => {
                self.hook(&event, pane, &payload).await;
                None
            }
            Request::Shutdown | Request::Detach | Request::Unknown => Some(Response::error(
                "this otag daemon cannot carry out that request yet",
            )),
        }
    }
}

impl Daemon {
    /// Act on what an agent's hook, or its status line, reported.
    ///
    /// A hook is the agent speaking for itself, so what it says outweighs
    /// any guess from its screen. Nothing here is trusted blindly: a payload
    /// that cannot be read, or one for a pane this daemon does not have, is
    /// logged at debug level and dropped.
    async fn hook(&self, event: &str, pane: Option<PaneId>, payload: &str) {
        let Some(pane) = pane else {
            // An agent started outside otag, or one whose environment was
            // not inherited. There is no pane to attribute this to.
            tracing::debug!(event, "a hook arrived without a pane");
            return;
        };
        if self.session.pane(pane).is_none() {
            tracing::debug!(event, %pane, "a hook arrived for a pane that is gone");
            return;
        }

        if event == hooks::STATUS_LINE {
            self.status_line(pane, payload).await;
            return;
        }

        let Some(hook) = hooks::HookPayload::parse(payload) else {
            tracing::debug!(event, %pane, "a hook payload could not be read");
            return;
        };
        if let Some(status) = hook.status(event) {
            self.session.set_status(pane, status, true).await;
        }
        if let Some(id) = hook.session_id.clone() {
            self.remember_session(pane, id, hook.transcript_path, hook.cwd)
                .await;
        }
    }

    /// Act on what the status line otag registered reported.
    ///
    /// It says nothing about what the agent is doing, but it is the one
    /// place the session identifier and the account's usage arrive from
    /// without otag having to read the agent's files.
    async fn status_line(&self, pane: PaneId, payload: &str) {
        let Some(line) = claude::StatusLine::parse(payload) else {
            tracing::debug!(%pane, "a status line payload could not be read");
            return;
        };
        if let Some(id) = line.session_id.clone() {
            self.remember_session(pane, id, line.transcript_path.clone(), line.cwd.clone())
                .await;
        }

        // The status line is the only place Claude Code says how much of the
        // account's limits is gone. Everything else otag shows about usage is
        // counted, and says so.
        if self.usage.record_limits(AgentKind::Claude, &line.limits()) {
            usage::publish(&self.session, &self.usage).await;
        }
    }

    /// Record which session a pane is running, and how to continue it.
    async fn remember_session(
        &self,
        pane: PaneId,
        id: String,
        transcript: Option<PathBuf>,
        cwd: Option<PathBuf>,
    ) {
        if let Some(path) = transcript.clone() {
            self.session.set_session_path(pane, path).await;
        }
        if !self.session.set_session_id(pane, id.clone()).await {
            // Already known, so the restore command is already right.
            return;
        }

        // Only Claude Code has hooks, so this is the agent that sent them.
        let reference = SessionRef {
            id,
            path: transcript.unwrap_or_default(),
            cwd,
        };
        let resume = AgentKind::Claude.adapter().resume_command(&reference);
        self.session.set_restore_command(pane, resume).await;
    }

    /// Attach a client, starting a workspace and a pane if there is none.
    async fn attach(&self, outbox: &Outbox, size: PaneSize, cwd: Option<&Path>) -> Response {
        match attach::start(&self.session, outbox, size, cwd).await {
            Ok(response) => response,
            Err(error) => {
                tracing::warn!(%error, "cannot attach a client to the session");
                Response::error(error.to_string())
            }
        }
    }

    /// Turn a failed structural change into an answer the user can act on.
    ///
    /// A change that worked needs no answer: every attached client is sent
    /// the new view anyway.
    fn report(&self, what: &str, result: otag_core::Result<()>) -> Option<Response> {
        match result {
            Ok(()) => None,
            Err(error) => {
                tracing::warn!(%error, what, "cannot carry out a request");
                Some(Response::error(format!("cannot {what}: {error}")))
            }
        }
    }
}

/// Completes when the operating system asks the daemon to terminate.
///
/// A signal handler that cannot be installed is logged and then ignored: the
/// daemon keeps serving and can still be stopped with `otag kill-server`.
async fn terminate_signal() {
    use tokio::signal::unix::{SignalKind, signal};

    let mut interrupt = match signal(SignalKind::interrupt()) {
        Ok(signal) => signal,
        Err(error) => {
            tracing::warn!(%error, "cannot listen for SIGINT");
            return std::future::pending().await;
        }
    };
    let mut terminate = match signal(SignalKind::terminate()) {
        Ok(signal) => signal,
        Err(error) => {
            tracing::warn!(%error, "cannot listen for SIGTERM");
            interrupt.recv().await;
            return;
        }
    };

    tokio::select! {
        _ = interrupt.recv() => tracing::info!("SIGINT received"),
        _ = terminate.recv() => tracing::info!("SIGTERM received"),
    }
}

#[cfg(test)]
mod tests {
    use otag_agents::PaneStatus;
    use otag_core::pane::PaneSize;

    use super::*;

    /// A daemon with one pane, which is what a hook needs to report against.
    async fn with_one_pane() -> (Daemon, PaneId) {
        let session = Session::new(PaneSpec::command(["sleep", "30"]));
        session
            .attach(PaneSize::new(24, 80), None)
            .await
            .expect("a pane");
        let pane = session
            .visible_panes()
            .first()
            .copied()
            .expect("the pane is there");
        (
            Daemon {
                session,
                usage: usage::Shared::new(),
            },
            pane,
        )
    }

    #[tokio::test]
    async fn a_hook_says_what_the_agent_is_doing() {
        let (daemon, pane) = with_one_pane().await;

        daemon
            .hook("Notification", Some(pane), r#"{"message":"Needs your ok"}"#)
            .await;

        let panes = daemon.session.agent_panes();
        let found = panes.first().expect("the pane");
        assert_eq!(found.status, PaneStatus::NeedsApproval);
        assert!(
            !found.trusts_the_screen,
            "an agent reporting for itself outranks its screen"
        );
    }

    #[tokio::test]
    async fn a_hook_that_carries_a_session_makes_the_pane_resume_it() {
        let (daemon, pane) = with_one_pane().await;

        daemon
            .hook(
                "SessionStart",
                Some(pane),
                r#"{"session_id":"abc-123","source":"startup"}"#,
            )
            .await;

        let summary = daemon
            .session
            .summaries()
            .into_iter()
            .next()
            .expect("the pane");
        assert_eq!(summary.session.as_deref(), Some("abc-123"));
        assert!(
            daemon
                .session
                .snapshot()
                .panes
                .iter()
                .any(|pane| pane.command == ["claude", "--resume", "abc-123"]),
            "the pane comes back by continuing the session"
        );
    }

    #[tokio::test]
    async fn the_status_line_carries_the_session_too() {
        let (daemon, pane) = with_one_pane().await;

        daemon
            .hook(
                otag_agents::hooks::STATUS_LINE,
                Some(pane),
                r#"{"session_id":"from-status-line","model":{"id":"x"}}"#,
            )
            .await;

        let summary = daemon
            .session
            .summaries()
            .into_iter()
            .next()
            .expect("the pane");
        assert_eq!(summary.session.as_deref(), Some("from-status-line"));
    }

    #[tokio::test]
    async fn a_hook_for_an_unknown_pane_changes_nothing() {
        let (daemon, pane) = with_one_pane().await;
        let unknown = pane.next().next();

        daemon.hook("Stop", Some(unknown), "{}").await;
        daemon.hook("Stop", None, "{}").await;

        let panes = daemon.session.agent_panes();
        assert_eq!(
            panes.first().map(|pane| pane.status),
            Some(PaneStatus::Idle)
        );
    }

    #[tokio::test]
    async fn a_payload_that_is_not_json_is_dropped_rather_than_believed() {
        let (daemon, pane) = with_one_pane().await;

        daemon
            .hook("Stop", Some(pane), "<html>not json</html>")
            .await;
        daemon
            .hook(otag_agents::hooks::STATUS_LINE, Some(pane), "")
            .await;

        let panes = daemon.session.agent_panes();
        let found = panes.first().expect("the pane");
        assert_eq!(found.status, PaneStatus::Idle);
        assert_eq!(found.session_id, None);
    }

    /// Where the redacted samples of real agent files live.
    const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures");

    #[tokio::test]
    async fn the_status_line_of_a_real_session_becomes_the_usage_report() {
        let (daemon, pane) = with_one_pane().await;
        let payload =
            std::fs::read_to_string(std::path::Path::new(FIXTURES).join("claude-statusline.json"))
                .expect("the fixture");

        daemon
            .hook(otag_agents::hooks::STATUS_LINE, Some(pane), &payload)
            .await;

        let report = daemon.usage.report();
        assert_eq!(report.rows.len(), 2, "both windows the agent reported");
        assert_eq!(
            report.rows[0].window,
            otag_features::usage::Window::FiveHour
        );
        assert_eq!(
            report.rows[0].used_pct(),
            70,
            "what the widget shows is what the agent said"
        );
        assert_eq!(
            report.rows[1].used_pct(),
            58,
            "the weekly window, rounded from the 57.99 the agent sent"
        );
        assert_eq!(
            report.level(),
            otag_features::usage::Level::Amber,
            "seventy per cent is worth knowing about"
        );

        // The clients are told without asking.
        assert_eq!(daemon.session.view().usage, report);
    }

    #[tokio::test]
    async fn a_status_line_from_an_account_without_limits_adds_no_numbers() {
        let (daemon, pane) = with_one_pane().await;
        let payload = std::fs::read_to_string(
            std::path::Path::new(FIXTURES).join("claude-statusline-no-usage.json"),
        )
        .expect("the fixture");

        daemon
            .hook(otag_agents::hooks::STATUS_LINE, Some(pane), &payload)
            .await;

        assert!(
            daemon.usage.report().is_empty(),
            "an account with no limits to report is not shown as zero per cent"
        );
    }
}
