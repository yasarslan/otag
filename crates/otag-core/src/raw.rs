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
//! A new replay follows dropped output, a resize, or a team role switching
//! to a replacement process. The pane's connection survives those changes.

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
/// Sends the replay before starting the task that pushes further output.
///
/// `size` is how large the client is drawing the pane. Giving one hands that
/// pane's size to the client: see [`Session::resize_pane_to`].
pub async fn attach_pane(
    session: &Arc<Session>,
    attachments: &Attachments,
    outbox: &Outbox,
    pane: PaneId,
    size: Option<PaneSize>,
) -> Result<()> {
    // Keep the handle if the program exits during attach, so the client
    // can still see the last screen it left behind.
    let mut handle = match session.pane(pane) {
        Some(handle) => handle,
        // Already over by the time the attach arrived, which is the normal
        // case for a program that prints one line and exits.
        None => session
            .ended_pane(pane)
            .ok_or(Error::PaneGone { id: Some(pane) })?,
    };

    // Resizing is for a pane that is still running; a pane that has ended
    // has nothing to resize, and the client is only being shown what it
    // left behind.
    if let Some(size) = size.filter(|_| !handle.is_finished()) {
        let resized = session.resize_pane_to(pane, size).await;
        if let Some(current) = session
            .pane(pane)
            .filter(|current| !current.same_process(&handle))
        {
            handle = current;
        } else if !handle.is_finished() {
            resized?;
        }
    }

    let attach = handle.attach_raw().await?;
    attachments.remove(outbox.id(), pane);
    if !outbox.send(screen_response(pane, &attach)).await {
        return Ok(());
    }

    let key = (outbox.id(), pane);
    let generation = attachments.generation();
    let (start, registered) = tokio::sync::oneshot::channel();
    let session = Arc::clone(session);
    let outbox = outbox.clone();
    let streams = attachments.clone();
    let task = tokio::spawn(async move {
        if registered.await.is_ok() {
            stream(session, handle, outbox, attach, streams, key, generation).await;
        }
    });
    attachments.insert(key, generation, task);
    let _ = start.send(());

    Ok(())
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
    session: Arc<Session>,
    mut handle: PaneHandle,
    outbox: Outbox,
    attach: RawAttach,
    attachments: Attachments,
    key: (u64, PaneId),
    generation: u64,
) {
    let pane = handle.id();
    let mut changes = session.changes();
    let mut attachment = Some(attach);
    loop {
        if let Some(attach) = attachment.take()
            && !stream_process(&handle, &outbox, attach).await
        {
            break;
        }
        // A role's PTY can end while the role remains on screen. Leave the
        // last display in place and wait for the same pane's next process.
        let next = loop {
            if let Some(next) = session
                .pane(pane)
                .filter(|next| !next.same_process(&handle))
            {
                break Some(next);
            }
            if !session.is_team_role(pane) {
                outbox
                    .send(Response::PaneEnded {
                        pane,
                        exit: handle.snapshot().exit.clone(),
                    })
                    .await;
                break None;
            }
            tokio::select! {
                () = outbox.closed() => break None,
                changed = changes.changed() => if changed.is_err() { break None; },
            }
        };
        let Some(next) = next else { break };
        handle = next;
        let Ok(replay) = handle.attach_raw().await else {
            // This process was itself replaced while the replay was being
            // built. Keep following the stable pane ID.
            continue;
        };
        if !outbox.send(screen_response(pane, &replay)).await {
            break;
        }
        attachment = Some(replay);
    }
    attachments.forget(key, generation);
}

/// Forward one PTY. True means it ended and the client is still listening.
async fn stream_process(handle: &PaneHandle, outbox: &Outbox, attach: RawAttach) -> bool {
    let pane = handle.id();
    let mut output = attach.output;
    let mut size = attach.size;
    let mut pane_state = handle.watch();
    let mut watching = true;

    loop {
        tokio::select! {
            // The client went away; nothing left to send it.
            () = outbox.closed() => return false,

            received = output.recv() => match received {
                Ok(bytes) => {
                    if !outbox.send(Response::Output { pane, bytes: Bytes(bytes.to_vec()) }).await {
                        return false;
                    }
                }
                Err(RecvError::Lagged(missed)) => {
                    // Output this client never saw has been dropped, so
                    // what it has on screen is wrong. A fresh replay is the
                    // only honest answer.
                    tracing::debug!(%pane, missed, "a raw client fell behind; replaying its screen");
                    match restart(handle, outbox, true).await {
                        Some(attach) => {
                            output = attach.output;
                            size = attach.size;
                        }
                        None => return handle.is_finished(),
                    }
                }
                // Every sender is gone, which happens when the pane's task
                // ends. A closed receiver has already handed over everything
                // that was buffered, so the program's last output has been
                // sent and the end can be reported.
                Err(RecvError::Closed) => {
                    return true;
                }
            },

            changed = pane_state.changed(), if watching => {
                if changed.is_err() {
                    watching = false;
                    continue;
                }

                let state = handle.snapshot();
                if state.screen.size() != size {
                    // The pane was resized, by this client or another one.
                    // Its old bytes assumed the old size, so the client is
                    // sent the screen again rather than a stream it would
                    // misread.
                    match restart(handle, outbox, false).await {
                        Some(attach) => {
                            output = attach.output;
                            size = attach.size;
                        }
                        None => return handle.is_finished(),
                    }
                }

                if state.exit.is_some() {
                    // Whatever the program printed last has to reach the
                    // client before the news that it ended.
                    drain(outbox, pane, &mut output).await;
                    return true;
                }
            }
        }
    }
}

/// Send the client the pane's screen again and follow it from there.
///
/// `None` once the pane or the client is gone, which ends the stream.
async fn restart(handle: &PaneHandle, outbox: &Outbox, history: bool) -> Option<RawAttach> {
    let attach = if history {
        handle.attach_raw().await
    } else {
        handle.replay_screen().await
    }
    .ok()?;
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
