//! Keeping a client's picture of the session up to date.
//!
//! Attaching gives the client a whole view: where the sidebar, the tab bar
//! and every visible pane go, and what is on each of those panes. After that
//! the daemon sends only what changed — a frame per pane that drew something,
//! and a fresh view whenever the layout itself moved.
//!
//! Each client has its own baseline, the last screen it was actually sent, so
//! a client that skips updates because it is behind still receives a correct
//! picture, and clients attached to the same session do not have to be in
//! step.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::frame::ScreenState;
use crate::pane::{PaneId, PaneSize};
use crate::proto::{Response, SessionView};
use crate::server::Outbox;
use crate::session::{PaneHandle, PaneState, Session};
use crate::{Error, Result};

/// How many "this pane drew something" notices may wait for one client.
///
/// The notices carry no screen data, only which pane to look at, so a client
/// that is behind collapses a burst into however many it gets round to.
const NOTICES: usize = 256;

/// Attach `outbox`'s client to the session and keep it up to date.
///
/// Returns the answer the client is waiting for; everything after it is sent
/// by a task of its own, which ends when the session has no panes left or
/// the client goes away.
pub async fn start(
    session: &Arc<Session>,
    outbox: &Outbox,
    size: PaneSize,
    cwd: Option<&Path>,
) -> Result<Response> {
    session.attach(size, cwd).await?;

    // Subscribed before the view is taken, so a change that lands between
    // the two is noticed rather than lost.
    let changes = session.changes();
    let view = session.view();
    if view.is_empty() {
        return Err(Error::NoPanes);
    }

    tokio::spawn(stream(
        Arc::clone(session),
        outbox.clone(),
        changes,
        view.clone(),
    ));
    Ok(Response::view(view))
}

/// Send one client everything that changes until there is nothing more to
/// say.
async fn stream(
    session: Arc<Session>,
    outbox: Outbox,
    mut changes: watch::Receiver<u64>,
    view: SessionView,
) {
    let (notices_tx, mut notices) = mpsc::channel(NOTICES);
    let mut following = Following::default();
    following.follow(&session, &view, &notices_tx);

    loop {
        tokio::select! {
            // The client went away. Stopping now releases the connection
            // instead of holding it until a pane next draws something.
            () = outbox.closed() => break,

            changed = changes.changed() => {
                if changed.is_err() {
                    // The session is gone, which only happens while the
                    // daemon is shutting down.
                    break;
                }
                let view = session.view();
                if view.is_empty() {
                    // Whatever the last program printed before it ended has
                    // to reach the client before the news that it did.
                    following.flush(&outbox).await;
                    outbox
                        .send(Response::SessionEnded { exit: session.last_exit() })
                        .await;
                    break;
                }
                if !outbox.send(Response::view(view.clone())).await {
                    break;
                }
                // The view carries every visible pane's whole screen, so it
                // is also the new baseline to diff against.
                following.follow(&session, &view, &notices_tx);
            }

            notice = notices.recv() => {
                let Some(pane) = notice else { break };
                if !following.send_frame(&outbox, pane).await {
                    break;
                }
            }
        }
    }

    following.stop();
}

/// One pane this client is being sent.
#[derive(Debug)]
struct Follow {
    /// Kept so that the pane's last screen is still reachable after the
    /// session has let go of it, which is what lets a program's final output
    /// be sent before the news that it ended.
    handle: PaneHandle,
    /// The screen this client has already been sent.
    sent: ScreenState,
    /// The task waking this client when the pane draws something.
    task: JoinHandle<()>,
}

/// The panes one client is being sent, and what it has already received.
#[derive(Debug, Default)]
struct Following {
    panes: HashMap<PaneId, Follow>,
}

impl Following {
    /// Follow exactly the panes `view` shows, with `view`'s screens as the
    /// baselines to diff against.
    fn follow(&mut self, session: &Session, view: &SessionView, notices: &mpsc::Sender<PaneId>) {
        // A pane that is no longer visible — another tab was selected, or it
        // closed — is not worth a task.
        self.panes.retain(|id, follow| {
            if view.panes.iter().any(|pane| pane.id == *id) {
                return true;
            }
            follow.task.abort();
            false
        });

        for pane in &view.panes {
            let mut screen = ScreenState::new(pane.area.size());
            if let Some(frame) = &pane.frame {
                screen.apply(frame);
            }

            match self.panes.get_mut(&pane.id) {
                Some(follow) => follow.sent = screen,
                None => {
                    let Some(handle) = session.pane(pane.id) else {
                        continue;
                    };
                    let task = tokio::spawn(notice(pane.id, handle.watch(), notices.clone()));
                    self.panes.insert(
                        pane.id,
                        Follow {
                            handle,
                            sent: screen,
                            task,
                        },
                    );
                }
            }
        }
    }

    /// Send what changed on one pane. Returns whether the client is still
    /// there.
    async fn send_frame(&mut self, outbox: &Outbox, pane: PaneId) -> bool {
        let Some(follow) = self.panes.get_mut(&pane) else {
            // Not a pane this client can see; its frames are not its
            // business.
            return true;
        };

        let screen = follow.handle.snapshot().screen.clone();
        let Some(frame) = follow.sent.diff(&screen) else {
            return true;
        };
        if !outbox.send(Response::Frame { pane, frame }).await {
            return false;
        }
        follow.sent = screen;
        true
    }

    /// Send whatever the followed panes have drawn and not yet sent.
    async fn flush(&mut self, outbox: &Outbox) {
        for pane in self.panes.keys().copied().collect::<Vec<_>>() {
            if !self.send_frame(outbox, pane).await {
                return;
            }
        }
    }

    /// Stop every task following a pane for this client.
    fn stop(self) {
        for follow in self.panes.into_values() {
            follow.task.abort();
        }
    }
}

/// Tell one client's stream that a pane drew something.
async fn notice(
    pane: PaneId,
    mut updates: watch::Receiver<Arc<PaneState>>,
    notices: mpsc::Sender<PaneId>,
) {
    loop {
        if updates.changed().await.is_err() {
            return;
        }
        if notices.send(pane).await.is_err() {
            return;
        }
    }
}
