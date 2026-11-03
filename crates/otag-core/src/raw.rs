//! Watching one pane as a stream of bytes.
//!
//! This is the other half of [`crate::attach`], for a client that has a
//! terminal emulator of its own — the macOS app, through SwiftTerm. Instead
//! of finished rows it is sent the pane's output, exactly as the program
//! wrote it, after one replay that puts the current screen in place
//! ([`crate::replay`]).
//!
//! The daemon still owns the pane. A raw client types through
//! [`crate::proto::Request::PaneInput`] and says how large it is drawing the
//! pane through [`crate::proto::Request::PaneResize`]; nothing about the
//! pane's program changes because of who is watching it.
//!
//! Two things can interrupt the stream, and both are answered with a new
//! replay rather than with silence: a client that falls far enough behind
//! that the daemon has dropped output it never received, and a pane that
//! changed size, whose old bytes no longer describe the screen.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::broadcast::error::{RecvError, TryRecvError};
use tokio::task::JoinHandle;

use crate::pane::{PaneId, PaneSize};
use crate::proto::{Bytes, Response};
use crate::server::Outbox;
use crate::session::{PaneHandle, RawAttach, Session};
use crate::{Error, Result};

/// The panes each client is watching in raw mode.
///
/// One task per client per pane pushes the output, and the client can ask
/// for it to stop; the key is the connection and the pane, which is exactly
/// what "this client is watching that pane" means.
#[derive(Debug, Clone, Default)]
pub struct Attachments {
    streams: Arc<Mutex<HashMap<(u64, PaneId), Stream>>>,
    /// Hands out the number that tells one stream from the stream that
    /// replaced it.
    next: Arc<std::sync::atomic::AtomicU64>,
}

/// One running stream, and the number identifying it.
#[derive(Debug)]
struct Stream {
    generation: u64,
    task: JoinHandle<()>,
}

impl Attachments {
    /// Nothing being watched.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// How many streams are running, across all clients.
    #[must_use]
    pub fn len(&self) -> usize {
        self.streams
            .lock()
            .map(|streams| streams.len())
            .unwrap_or(0)
    }

    /// Whether nobody is watching anything in raw mode.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Remember a stream, replacing — and stopping — one for the same client
    /// and pane.
    ///
    /// Attaching twice is not an error: a client that lost track of its
    /// stream, or that re-attached after a resize, gets one stream either
    /// way rather than two sending the same bytes.
    fn insert(&self, key: (u64, PaneId), generation: u64, task: JoinHandle<()>) {
        let Ok(mut streams) = self.streams.lock() else {
            // The lock is poisoned, which means a task panicked while
            // holding it. The stream still runs; it just cannot be stopped
            // by name, and it stops on its own when the client goes away.
            return;
        };
        if let Some(previous) = streams.insert(key, Stream { generation, task }) {
            previous.task.abort();
        }
    }

    /// The number for a stream about to start.
    fn generation(&self) -> u64 {
        self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    /// Stop one client's stream for one pane, and say whether there was one.
    pub fn remove(&self, client: u64, pane: PaneId) -> bool {
        let Ok(mut streams) = self.streams.lock() else {
            return false;
        };
        match streams.remove(&(client, pane)) {
            Some(stream) => {
                stream.task.abort();
                true
            }
            None => false,
        }
    }

    /// Forget a stream that has ended by itself, without stopping whatever
    /// has taken its place.
    fn forget(&self, key: (u64, PaneId), generation: u64) {
        let Ok(mut streams) = self.streams.lock() else {
            return;
        };
        if streams
            .get(&key)
            .is_some_and(|current| current.generation == generation)
        {
            streams.remove(&key);
        }
    }

    /// Stop every stream of one client, because its connection ended.
    pub fn remove_client(&self, client: u64) {
        let Ok(mut streams) = self.streams.lock() else {
            return;
        };
        streams.retain(|(id, _), stream| {
            if *id == client {
                stream.task.abort();
                return false;
            }
            true
        });
    }
}

/// Start sending `outbox`'s client the bytes of one pane.
///
/// Returns the first message, which carries the screen as it is now; the
/// output after it is pushed by a task that ends when the pane ends or the
/// client goes away.
///
/// `size` is how large the client is drawing the pane. Giving one hands that
/// pane's size to the client: see [`Session::resize_pane_to`].
pub async fn attach_pane(
    session: &Arc<Session>,
    attachments: &Attachments,
    outbox: &Outbox,
    pane: PaneId,
    size: Option<PaneSize>,
) -> Result<Response> {
    if let Some(size) = size {
        session.resize_pane_to(pane, size).await?;
    }

    let handle = session
        .pane(pane)
        .ok_or(Error::PaneGone { id: Some(pane) })?;
    let attach = handle.attach_raw().await?;
    let first = screen_response(pane, &attach);

    let key = (outbox.id(), pane);
    let generation = attachments.generation();
    let task = tokio::spawn(stream(
        handle,
        outbox.clone(),
        attach,
        attachments.clone(),
        key,
        generation,
    ));
    attachments.insert(key, generation, task);

    Ok(first)
}

/// The message that puts a pane's whole screen on a client's terminal.
fn screen_response(pane: PaneId, attach: &RawAttach) -> Response {
    Response::PaneScreen {
        pane,
        size: attach.size,
        bytes: Bytes(attach.snapshot.clone()),
    }
}

/// Push one pane's output to one client until there is nothing more to send.
async fn stream(
    handle: PaneHandle,
    outbox: Outbox,
    attach: RawAttach,
    attachments: Attachments,
    key: (u64, PaneId),
    generation: u64,
) {
    let pane = handle.id();
    let mut output = attach.output;
    let mut size = attach.size;
    let mut pane_state = handle.watch();

    loop {
        tokio::select! {
            // The client went away; nothing left to send it.
            () = outbox.closed() => break,

            received = output.recv() => match received {
                Ok(bytes) => {
                    if !outbox.send(Response::Output { pane, bytes: Bytes(bytes.to_vec()) }).await {
                        break;
                    }
                }
                Err(RecvError::Lagged(missed)) => {
                    // Output this client never saw has been dropped, so
                    // what it has on screen is wrong. A fresh replay is the
                    // only honest answer.
                    tracing::debug!(%pane, missed, "a raw client fell behind; replaying its screen");
                    match restart(&handle, &outbox).await {
                        Some(attach) => {
                            output = attach.output;
                            size = attach.size;
                        }
                        None => break,
                    }
                }
                // Every sender is gone, which happens when the pane's task
                // ends. A closed receiver has already handed over everything
                // that was buffered, so the program's last output has been
                // sent and the end can be reported.
                Err(RecvError::Closed) => {
                    let exit = handle.snapshot().exit.clone();
                    outbox.send(Response::PaneEnded { pane, exit }).await;
                    break;
                }
            },

            changed = pane_state.changed() => {
                if changed.is_err() {
                    break;
                }

                let state = handle.snapshot();
                if state.screen.size() != size {
                    // The pane was resized, by this client or another one.
                    // Its old bytes assumed the old size, so the client is
                    // sent the screen again rather than a stream it would
                    // misread.
                    match restart(&handle, &outbox).await {
                        Some(attach) => {
                            output = attach.output;
                            size = attach.size;
                        }
                        None => break,
                    }
                }

                if let Some(exit) = state.exit.clone() {
                    // Whatever the program printed last has to reach the
                    // client before the news that it ended.
                    drain(&outbox, pane, &mut output).await;
                    outbox.send(Response::PaneEnded { pane, exit: Some(exit) }).await;
                    break;
                }
            }
        }
    }

    attachments.forget(key, generation);
}

/// Send the client the pane's screen again and follow it from there.
///
/// `None` once the pane or the client is gone, which ends the stream.
async fn restart(handle: &PaneHandle, outbox: &Outbox) -> Option<RawAttach> {
    let attach = handle.attach_raw().await.ok()?;
    if !outbox.send(screen_response(handle.id(), &attach)).await {
        return None;
    }
    Some(attach)
}

/// Send whatever output is already waiting, without waiting for more.
async fn drain(
    outbox: &Outbox,
    pane: PaneId,
    output: &mut tokio::sync::broadcast::Receiver<Arc<[u8]>>,
) {
    loop {
        match output.try_recv() {
            Ok(bytes) => {
                if !outbox
                    .send(Response::Output {
                        pane,
                        bytes: Bytes(bytes.to_vec()),
                    })
                    .await
                {
                    return;
                }
            }
            Err(TryRecvError::Lagged(_)) => {}
            Err(TryRecvError::Empty | TryRecvError::Closed) => return,
        }
    }
}
