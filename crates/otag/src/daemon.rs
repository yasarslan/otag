//! The daemon: owns the socket, the panes, and the programs running on them.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use otag_core::attach;
use otag_core::pane::{PaneId, PaneSize, PaneSpec};
use otag_core::paths::{Dirs, SOCKET_ENV};
use otag_core::proto::{PaneInfo, Request, Response};
use otag_core::server::{Handler, Outbox, Server};
use otag_core::session::Session;

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
    let daemon = Daemon {
        session: Arc::new(Session::new()),
        socket: socket.to_path_buf(),
    };
    let result = server.run_until(daemon, terminate_signal()).await;
    tracing::info!("otag daemon stopped");
    result.context("the otag daemon stopped with an error")
}

/// Everything one daemon owns.
struct Daemon {
    session: Arc<Session>,
    socket: PathBuf,
}

impl Handler for Daemon {
    /// Answer the requests this milestone supports.
    ///
    /// `Shutdown` and `Detach` never reach here: the server handles its own
    /// lifecycle.
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
                        exit: pane.exit,
                    })
                    .collect(),
            }),
            Request::Attach { size, cwd } => Some(self.attach(outbox, size, cwd.as_deref()).await),
            Request::Input { bytes } => {
                if let Some(pane) = self.attached_pane()
                    && let Err(error) = attach::input(&self.session, pane, bytes).await
                {
                    // A key pressed as a program exits has nowhere to go.
                    // The client learns the pane ended from its own stream.
                    tracing::debug!(%error, "cannot deliver input to a pane");
                }
                None
            }
            Request::Resize { size } => {
                if let Some(pane) = self.attached_pane()
                    && let Err(error) = attach::resize(&self.session, pane, size).await
                {
                    tracing::debug!(%error, "cannot resize a pane");
                }
                None
            }
            Request::Shutdown | Request::Detach | Request::Unknown => Some(Response::error(
                "this otag daemon cannot carry out that request yet",
            )),
        }
    }
}

impl Daemon {
    /// Attach a client to a pane, starting one if there is none.
    async fn attach(&self, outbox: &Outbox, size: PaneSize, cwd: Option<&Path>) -> Response {
        let spec = self.pane_spec(cwd);
        match attach::start(&self.session, outbox, &spec, size).await {
            Ok(response) => response,
            Err(error) => {
                tracing::warn!(%error, "cannot attach a client to a pane");
                Response::error(error.to_string())
            }
        }
    }

    /// What a new pane runs: the user's shell, told how to reach otag.
    fn pane_spec(&self, cwd: Option<&Path>) -> PaneSpec {
        PaneSpec::shell()
            .with_cwd_opt(cwd)
            .with_env(SOCKET_ENV, &self.socket)
    }

    /// The pane a client's keys belong to.
    ///
    /// This milestone runs one pane, so there is nothing to choose between.
    /// Once a layout can hold several, the pane a client is attached to
    /// becomes part of the connection instead of being looked up here.
    fn attached_pane(&self) -> Option<PaneId> {
        self.session
            .summaries()
            .into_iter()
            .find(|pane| pane.exit.is_none())
            .map(|pane| pane.id)
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
