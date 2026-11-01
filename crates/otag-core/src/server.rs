//! The daemon's socket server.
//!
//! [`Server::bind`] takes ownership of the Unix socket — including cleaning up
//! a socket left behind by a crashed daemon — and [`Server::run_until`] serves
//! newline-delimited JSON requests until a shutdown is requested, either by a
//! client sending [`Request::Shutdown`] or by the caller's own shutdown future
//! completing.
//!
//! A single bad line, a single unparsable request or a single dropped
//! connection never stops the server: it is logged and the loop continues.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Notify;

use crate::proto::{self, Request, Response};
use crate::{Error, Result};

/// Permissions of the socket and its directory: accessible to the owner only.
const SOCKET_MODE: u32 = 0o600;
const DIR_MODE: u32 = 0o700;

/// Answers requests that are not part of the server's own lifecycle.
///
/// Implementations must not block: they run on the runtime that also serves
/// every other connection.
pub trait Handler: Send + Sync + 'static {
    /// Produce an answer for `request`.
    fn handle(&self, request: Request) -> Response;
}

impl<F> Handler for F
where
    F: Fn(Request) -> Response + Send + Sync + 'static,
{
    fn handle(&self, request: Request) -> Response {
        self(request)
    }
}

/// A bound otag daemon socket.
#[derive(Debug)]
pub struct Server {
    listener: UnixListener,
    path: PathBuf,
    shutdown: Arc<Notify>,
}

impl Server {
    /// Bind the daemon socket at `path`.
    ///
    /// Creates the parent directory if needed. If a socket file is already
    /// present, a live daemon is detected by connecting to it: if one answers,
    /// this returns [`Error::AlreadyRunning`]; if nothing answers, the stale
    /// file is removed and the bind proceeds.
    pub async fn bind(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();

        if let Some(parent) = path.parent() {
            create_private_dir(parent)?;
        }
        reclaim_stale_socket(&path).await?;

        let listener = UnixListener::bind(&path)
            .map_err(|source| Error::io(format!("cannot bind the otag socket {path:?}"), source))?;
        set_mode(&path, SOCKET_MODE)?;

        tracing::info!(socket = %path.display(), "otag daemon listening");
        Ok(Self {
            listener,
            path,
            shutdown: Arc::new(Notify::new()),
        })
    }

    /// Path this server is listening on.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// A handle that can ask the server to stop from anywhere.
    pub fn shutdown_handle(&self) -> ShutdownHandle {
        ShutdownHandle {
            notify: Arc::clone(&self.shutdown),
        }
    }

    /// Serve requests until a shutdown is requested or `shutdown` completes.
    ///
    /// Accept errors are logged and retried, so a transient failure does not
    /// take the daemon down.
    pub async fn run_until<H: Handler>(
        self,
        handler: H,
        shutdown: impl Future<Output = ()>,
    ) -> Result<()> {
        let handler = Arc::new(handler);
        let notify = Arc::clone(&self.shutdown);
        let mut shutdown = std::pin::pin!(shutdown);
        // Registered once, before the first connection is accepted, and kept
        // alive across iterations: a shutdown requested while the loop is busy
        // elsewhere must not be missed.
        let mut requested = std::pin::pin!(notify.notified());

        loop {
            tokio::select! {
                () = &mut requested => {
                    tracing::info!("shutdown requested, otag daemon stopping");
                    return Ok(());
                }
                () = &mut shutdown => {
                    tracing::info!("shutdown signal received, otag daemon stopping");
                    return Ok(());
                }
                accepted = self.listener.accept() => match accepted {
                    Ok((stream, _addr)) => {
                        let handler = Arc::clone(&handler);
                        let notify = Arc::clone(&notify);
                        tokio::spawn(async move {
                            if let Err(error) = serve_connection(stream, handler, notify).await {
                                tracing::warn!(%error, "otag connection ended with an error");
                            }
                        });
                    }
                    Err(error) => {
                        tracing::warn!(%error, "cannot accept an otag connection");
                    }
                },
            }
        }
    }

    /// Serve requests until a client asks the daemon to shut down.
    pub async fn run<H: Handler>(self, handler: H) -> Result<()> {
        self.run_until(handler, std::future::pending()).await
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // Leaving the socket behind would make the next start look like a
        // crashed daemon, so remove it. A failure here is not worth an error.
        if let Err(error) = std::fs::remove_file(&self.path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::debug!(%error, socket = %self.path.display(), "cannot remove the otag socket");
            }
        }
    }
}

/// Asks a running [`Server`] to stop.
#[derive(Debug, Clone)]
pub struct ShutdownHandle {
    notify: Arc<Notify>,
}

impl ShutdownHandle {
    /// Request shutdown. Returns immediately; the server stops on its next
    /// loop iteration.
    ///
    /// The request is remembered, so it also takes effect when it arrives
    /// before the server has started serving.
    pub fn shutdown(&self) {
        self.notify.notify_one();
    }
}

/// Read requests from one connection and write one response per line.
async fn serve_connection<H: Handler>(
    stream: UnixStream,
    handler: Arc<H>,
    shutdown: Arc<Notify>,
) -> Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let mut lines = BufReader::new(read_half).lines();

    loop {
        let line = match lines.next_line().await {
            Ok(Some(line)) => line,
            Ok(None) => return Ok(()),
            Err(source) => return Err(Error::io("cannot read from an otag client", source)),
        };
        if line.trim().is_empty() {
            continue;
        }

        let (response, stop) = match proto::decode_line::<Request>(&line) {
            Ok(Request::Shutdown) => (Response::Ok, true),
            Ok(Request::Unknown) => (
                Response::error(
                    "this otag daemon does not support that request. \
                     Run `otag kill-server` and start again to pick up the new version.",
                ),
                false,
            ),
            Ok(request) => (handler.handle(request), false),
            Err(error) => {
                tracing::debug!(%error, "ignoring a malformed otag request");
                (
                    Response::error(format!("malformed request: {error}")),
                    false,
                )
            }
        };

        let encoded = proto::encode_line(&response)?;
        if let Err(source) = write_half.write_all(encoded.as_bytes()).await {
            return Err(Error::io("cannot answer an otag client", source));
        }
        if let Err(source) = write_half.flush().await {
            return Err(Error::io("cannot answer an otag client", source));
        }

        if stop {
            shutdown.notify_one();
            return Ok(());
        }
    }
}

/// Remove a socket file that no daemon is listening on.
async fn reclaim_stale_socket(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    if UnixStream::connect(path).await.is_ok() {
        return Err(Error::AlreadyRunning {
            path: path.to_path_buf(),
        });
    }
    tracing::info!(socket = %path.display(), "removing a socket left behind by an earlier daemon");
    std::fs::remove_file(path)
        .map_err(|source| Error::io(format!("cannot remove the stale socket {path:?}"), source))
}

fn create_private_dir(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)
        .map_err(|source| Error::io(format!("cannot create the directory {dir:?}"), source))?;
    set_mode(dir, DIR_MODE)
}

fn set_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).map_err(|source| {
        Error::io(
            format!("cannot set permissions {mode:o} on {path:?}"),
            source,
        )
    })
}
