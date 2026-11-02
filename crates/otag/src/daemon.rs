//! The daemon: owns the socket, the panes, and the programs running on them.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use otag_core::attach;
use otag_core::pane::{PaneSize, PaneSpec};
use otag_core::paths::{Dirs, SOCKET_ENV};
use otag_core::proto::{PaneInfo, Request, Response};
use otag_core::server::{Handler, Outbox, Server};
use otag_core::session::Session;

use crate::persist;

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

    let daemon = Daemon {
        session: Arc::clone(&session),
    };
    let result = server.run_until(daemon, terminate_signal()).await;

    // The panes are about to be killed with the daemon, so what they show
    // now is what a restart has to put back.
    if let Some(snapshots) = &snapshots {
        if let Some(saving) = saving {
            saving.abort();
        }
        persist::save_everything(&session, snapshots).await;
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
                        exit: pane.exit,
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
            Request::Shutdown | Request::Detach | Request::Unknown => Some(Response::error(
                "this otag daemon cannot carry out that request yet",
            )),
        }
    }
}

impl Daemon {
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
