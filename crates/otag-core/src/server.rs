//! The daemon's socket server.
//!
//! [`Server::bind`] takes ownership of the Unix socket — including cleaning up
//! a socket left behind by a crashed daemon — and [`Server::run_until`] serves
//! newline-delimited JSON requests until a shutdown is requested, either by a
//! client sending [`Request::Shutdown`] or by the caller's own shutdown future
//! completing.
//!
//! A connection starts as one request, one response. Once a client attaches
//! to a pane it becomes two-way: the client keeps sending keys and resizes
//! while the daemon pushes frames, in both directions at once. Everything the
//! daemon sends goes through that connection's [`Outbox`], which keeps the
//! order of what a client receives and bounds how much may wait for it.
//!
//! A single bad line, a single unparsable request or a single dropped
//! connection never stops the server: it is logged and the loop continues.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Notify, mpsc, watch};

use crate::proto::{self, Request, Response};
use crate::{Error, Result};

/// Permissions of the socket and its directory: accessible to the owner only.
const SOCKET_MODE: u32 = 0o600;
const DIR_MODE: u32 = 0o700;

/// How many messages may wait to be written to one client.
///
/// Reaching this means the client is not reading. Senders wait rather than
/// queue without bound, which is what makes a slow client skip frames
/// instead of growing the daemon's memory.
const OUTBOX_QUEUE: usize = 64;

/// Hands out the identifier of each connection.
///
/// Counting up is enough: an identifier is only ever compared with another,
/// and a daemon would have to accept a connection every nanosecond for
/// longer than it will run to come near the limit.
static NEXT_CONNECTION: AtomicU64 = AtomicU64::new(1);

/// How long a finished connection waits for what is already queued to be
/// written before the writing task is dropped.
///
/// A client that asked to detach should still receive the answer; a client
/// that vanished cannot receive anything, and must not keep a task alive.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(1);

/// Everything the daemon sends to one client, in order.
///
/// A handler keeps a clone to push messages that are not answers to a
/// request, such as the frames of an attached pane. Such a task must stop
/// when the connection ends, which it learns from [`Outbox::closed`];
/// otherwise it would keep the connection's writing task alive with nobody
/// to write to.
#[derive(Debug, Clone)]
pub struct Outbox {
    /// Which connection this is, so that a handler keeping something per
    /// client — a pane a client is watching, say — can tell them apart.
    id: u64,
    messages: mpsc::Sender<Response>,
    finished: watch::Receiver<bool>,
}

impl Outbox {
    /// Which connection this outbox belongs to.
    ///
    /// Unique within a daemon run, and never reused, so a handler may use it
    /// as the key of anything it keeps for one client.
    #[must_use]
    pub const fn id(&self) -> u64 {
        self.id
    }

    /// Queue a message, waiting while the client is behind.
    ///
    /// Returns `false` once the client is gone, which is the signal for a
    /// task pushing frames to stop.
    pub async fn send(&self, response: Response) -> bool {
        if !self.is_open() {
            return false;
        }
        self.messages.send(response).await.is_ok()
    }

    /// Whether the client is still connected.
    #[must_use]
    pub fn is_open(&self) -> bool {
        !self.messages.is_closed() && !*self.finished.borrow()
    }

    /// Completes once the connection has ended.
    ///
    /// A task pushing messages should wait on this alongside its own work,
    /// so that it stops as soon as the client is gone rather than when it
    /// next has something to send.
    pub async fn closed(&self) {
        let mut finished = self.finished.clone();
        // `wait_for` checks the value it already has first, so a connection
        // that ended before this call is noticed immediately. An error means
        // the connection's task is gone, which also means closed.
        let _ = finished.wait_for(|finished| *finished).await;
    }
}

/// Answers requests that are not part of the server's own lifecycle.
///
/// A handler may answer straight away by returning a response, answer
/// nothing — a key press needs no round trip — or keep the `outbox` to send
/// messages later.
///
/// Implementations run on the runtime that also serves every other
/// connection, so they must wait only on otag's own channels, never on a
/// blocking call.
pub trait Handler: Send + Sync + 'static {
    /// Produce an answer for `request`, if it needs one.
    fn handle(
        &self,
        request: Request,
        outbox: &Outbox,
    ) -> impl Future<Output = Option<Response>> + Send;
}

impl<F> Handler for F
where
    F: Fn(Request) -> Response + Send + Sync + 'static,
{
    async fn handle(&self, request: Request, _outbox: &Outbox) -> Option<Response> {
        Some(self(request))
    }
}

/// A bound otag daemon socket.
#[derive(Debug)]
pub struct Server {
    listener: UnixListener,
    path: PathBuf,
    identity: std::fs::Metadata,
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
        let identity = std::fs::metadata(&path)
            .map_err(|source| Error::io("cannot inspect the daemon socket", source))?;

        tracing::info!(socket = %path.display(), "otag daemon listening");
        Ok(Self {
            listener,
            path,
            identity,
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
        &self,
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
        let (closing, closed) = watch::channel(false);
        let mut connections = tokio::task::JoinSet::new();

        loop {
            tokio::select! {
                () = &mut requested => {
                    tracing::info!("shutdown requested, otag daemon stopping");
                    break;
                }
                () = &mut shutdown => {
                    tracing::info!("shutdown signal received, otag daemon stopping");
                    break;
                }
                accepted = self.listener.accept() => match accepted {
                    Ok((stream, _addr)) => {
                        let handler = Arc::clone(&handler);
                        let notify = Arc::clone(&notify);
                        let closed = closed.clone();
                        connections.spawn(async move {
                            if let Err(error) = serve_connection(stream, handler, notify, closed).await {
                                tracing::warn!(%error, "otag connection ended with an error");
                            }
                        });
                    }
                    Err(error) => {
                        tracing::warn!(%error, "cannot accept an otag connection");
                    }
                },
                _ = connections.join_next(), if !connections.is_empty() => {},
            }
        }
        // Close client streams before the daemon stops pane processes. A
        // shutdown is a lost connection, not an ordinary pane-ended event.
        let _ = closing.send(true);
        if tokio::time::timeout(DRAIN_TIMEOUT + Duration::from_millis(100), async {
            while connections.join_next().await.is_some() {}
        })
        .await
        .is_err()
        {
            connections.shutdown().await;
        }
        Ok(())
    }

    /// Serve requests until a client asks the daemon to shut down.
    pub async fn run<H: Handler>(self, handler: H) -> Result<()> {
        self.run_until(handler, std::future::pending()).await
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        use std::os::unix::fs::MetadataExt;
        if !std::fs::metadata(&self.path)
            .is_ok_and(|now| now.ino() == self.identity.ino() && now.dev() == self.identity.dev())
        {
            return;
        }
        // Leaving the socket behind would make the next start look like a
        // crashed daemon, so remove it. A failure here is not worth an error.
        if let Err(error) = std::fs::remove_file(&self.path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::debug!(%error, socket = %self.path.display(), "cannot remove the otag socket");
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

/// Read requests from one connection while writing everything the daemon
/// has for it.
///
/// Reading and writing are separate tasks so that a client typing while a
/// pane produces output does not have to wait for either direction.
async fn serve_connection<H: Handler>(
    stream: UnixStream,
    handler: Arc<H>,
    shutdown: Arc<Notify>,
    mut closing: watch::Receiver<bool>,
) -> Result<()> {
    let (read_half, write_half) = stream.into_split();
    let (messages_tx, messages_rx) = mpsc::channel(OUTBOX_QUEUE);
    let (finished_tx, finished_rx) = watch::channel(false);
    let outbox = Outbox {
        id: NEXT_CONNECTION.fetch_add(1, Ordering::Relaxed),
        messages: messages_tx,
        finished: finished_rx,
    };
    let mut writer = std::pin::pin!(write_messages(write_half, messages_rx));

    let result = tokio::select! {
        result = read_requests(read_half, handler, shutdown, &outbox) => result,
        _ = closing.changed() => Ok(()),
        result = &mut writer => return result,
    };

    // Tell everything pushing messages to this client to stop, then drop the
    // last outbox so the writer sees the end of the queue.
    let _ = finished_tx.send(true);
    drop(outbox);

    // Wait only briefly: what is queued for a client that is still there
    // gets written, and a task that ignores the end of the connection cannot
    // hold this one open.
    match tokio::time::timeout(DRAIN_TIMEOUT, &mut writer).await {
        Ok(Err(error)) => return Err(error),
        Ok(_) => {}
        Err(_) => {
            tracing::debug!("giving up on writing the rest of a client's messages");
        }
    }
    result
}

/// Handle one client's requests until it goes away.
async fn read_requests<H: Handler>(
    read_half: tokio::net::unix::OwnedReadHalf,
    handler: Arc<H>,
    shutdown: Arc<Notify>,
    outbox: &Outbox,
) -> Result<()> {
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

        // Requests about the connection or the daemon itself are answered
        // here; everything else belongs to the handler.
        let (response, stop) = match proto::decode_line::<Request>(&line) {
            Ok(Request::Shutdown) => (Some(Response::Ok), Stop::Daemon),
            Ok(Request::Detach) => (Some(Response::Ok), Stop::Connection),
            Ok(Request::Unknown) => (
                Some(Response::error(
                    "this otag daemon does not support that request. \
                     Run `otag kill-server` and start again to pick up the new version.",
                )),
                Stop::No,
            ),
            Ok(request) => (handler.handle(request, outbox).await, Stop::No),
            Err(error) => {
                tracing::debug!(%error, "ignoring a malformed otag request");
                (
                    Some(Response::error(format!("malformed request: {error}"))),
                    Stop::No,
                )
            }
        };

        if let Some(response) = response
            && !outbox.send(response).await
        {
            // The client stopped reading; nothing left to serve it.
            return Ok(());
        }

        match stop {
            Stop::No => {}
            Stop::Connection => return Ok(()),
            Stop::Daemon => {
                shutdown.notify_one();
                return Ok(());
            }
        }
    }
}

/// What a request ends, beyond itself.
enum Stop {
    /// Nothing; keep serving this client.
    No,
    /// This connection, leaving the daemon running.
    Connection,
    /// The whole daemon.
    Daemon,
}

/// Write queued messages to the client, one JSON line each.
async fn write_messages(
    mut write_half: tokio::net::unix::OwnedWriteHalf,
    mut messages: mpsc::Receiver<Response>,
) -> Result<()> {
    while let Some(response) = messages.recv().await {
        let encoded = proto::encode_line(&response)?;
        if let Err(source) = write_half.write_all(encoded.as_bytes()).await {
            // A client that closed mid-frame is normal, not a failure worth
            // reporting up.
            tracing::debug!(%source, "cannot write to an otag client");
            return Ok(());
        }
        if let Err(source) = write_half.flush().await {
            tracing::debug!(%source, "cannot write to an otag client");
            return Ok(());
        }
    }
    Ok(())
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
