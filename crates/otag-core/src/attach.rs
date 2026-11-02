//! Keeping a client's view of a pane up to date.
//!
//! Attaching gives the client the pane's whole screen once; after that the
//! daemon sends only what changed. Each client has its own baseline — the
//! last screen it was actually sent — so a client that skips updates because
//! it is behind still receives a correct picture, and clients attached to the
//! same pane do not have to be in step.

use std::sync::Arc;

use tokio::sync::watch;

use crate::frame::ScreenState;
use crate::pane::{PaneId, PaneSize, PaneSpec};
use crate::proto::Response;
use crate::server::Outbox;
use crate::session::{PaneView, Session};
use crate::{Error, Result};

/// Attach `outbox`'s client to a pane and keep sending it frames.
///
/// Returns the answer the client is waiting for; the frames that follow are
/// sent by a task of their own, which ends when the pane's program ends or
/// the client goes away.
pub async fn start(
    session: &Session,
    outbox: &Outbox,
    spec: &PaneSpec,
    size: PaneSize,
) -> Result<Response> {
    let pane = session.attach_pane(spec, size)?;

    // A client attaching to a pane that already exists decides the size: it
    // is the one drawing it.
    if pane.view().screen.size() != size
        && let Err(error) = pane.resize(size).await
    {
        tracing::debug!(%error, pane = %pane.id(), "cannot resize a pane on attach");
    }

    let view = pane.view();
    let frame = view.screen.to_frame();
    let attached = Response::Attached {
        pane: pane.id(),
        frame,
    };

    tokio::spawn(stream(
        pane.id(),
        pane.watch(),
        outbox.clone(),
        view.screen.clone(),
    ));

    Ok(attached)
}

/// Send one client every change to a pane until there is nothing more to say.
async fn stream(
    pane: PaneId,
    mut updates: watch::Receiver<Arc<PaneView>>,
    outbox: Outbox,
    mut sent: ScreenState,
) {
    loop {
        // Take the newest screen, not the next one in a queue: an update the
        // client was too slow to be sent is superseded, never replayed.
        let view = Arc::clone(&updates.borrow_and_update());

        if let Some(frame) = sent.diff(&view.screen) {
            if !outbox.send(Response::Frame { pane, frame }).await {
                return;
            }
            sent = view.screen.clone();
        }

        if let Some(exit) = view.exit.clone() {
            outbox.send(Response::PaneExited { pane, exit }).await;
            return;
        }

        tokio::select! {
            // The client went away. Stopping now releases the connection
            // instead of holding it until the pane next draws something.
            () = outbox.closed() => return,
            changed = updates.changed() => if changed.is_err() {
                // The pane is gone without having reported an exit, which
                // can only happen while the daemon is shutting down.
                return;
            },
        }
    }
}

/// Write bytes to the pane a client is attached to.
pub async fn input(session: &Session, pane: PaneId, bytes: Vec<u8>) -> Result<()> {
    session
        .pane(pane)
        .ok_or(Error::PaneGone { id: Some(pane) })?
        .input(bytes)
        .await
}

/// Tell a pane that the client drawing it changed size.
pub async fn resize(session: &Session, pane: PaneId, size: PaneSize) -> Result<()> {
    session
        .pane(pane)
        .ok_or(Error::PaneGone { id: Some(pane) })?
        .resize(size)
        .await
}
