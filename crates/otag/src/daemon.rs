//! The daemon: owns the socket and, from the next milestone on, every pane.

use std::path::Path;

use anyhow::{Context, Result};
use otag_core::paths::Dirs;
use otag_core::proto::{Request, Response};
use otag_core::server::Server;

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
    let result = server.run_until(handle, terminate_signal()).await;
    tracing::info!("otag daemon stopped");
    result.context("the otag daemon stopped with an error")
}

/// Answer the requests this milestone supports.
///
/// `Shutdown` never reaches here: the server handles its own lifecycle.
fn handle(request: Request) -> Response {
    match request {
        Request::Ping => Response::Pong,
        Request::Version => Response::Version {
            version: otag_core::VERSION.to_owned(),
        },
        Request::Shutdown | Request::Unknown => {
            Response::error("this otag daemon cannot carry out that request yet")
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
