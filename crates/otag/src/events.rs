//! Telling subscribed clients what is happening, without being asked.
//!
//! The terminal client asks for a view and is sent frames; a client with a
//! user interface of its own wants something else — that a pane's agent is
//! now waiting, that the limits moved, that a handoff finished. One
//! subscription carries all of it, pushed as it happens, so the app never
//! polls.
//!
//! Two sources feed it. Anything the session already knows about — panes,
//! statuses, usage — is read from the session whenever its generation moves,
//! which is also what keeps two clients from disagreeing. Everything else is
//! published on a [`Bus`] by whatever noticed it.

use std::sync::Arc;

use otag_core::proto::{Event, PaneInfo, Response};
use otag_core::server::Outbox;
use otag_core::session::Session;
use tokio::sync::broadcast;

/// How many events may wait for one subscribed client.
///
/// Reaching this means the client is not reading. It is then sent the state
/// of the session as it is now rather than the events it missed, which is
/// both cheaper and more correct.
const QUEUE: usize = 64;

/// Carries events from whatever noticed them to every subscribed client.
#[derive(Debug, Clone)]
pub struct Bus {
    sender: broadcast::Sender<Event>,
}

impl Default for Bus {
    fn default() -> Self {
        Self::new()
    }
}

impl Bus {
    /// A bus with nobody listening.
    #[must_use]
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(QUEUE);
        Self { sender }
    }

    /// Tell every subscribed client about something.
    ///
    /// Nothing is reported when nobody is listening: an event with no
    /// subscriber is not a failure.
    pub fn publish(&self, event: Event) {
        let _ = self.sender.send(event);
    }

    /// Whether any client is listening.
    ///
    /// Used to decide who tells the user about a waiting agent: a client
    /// with a window of its own does it better than the daemon can.
    #[must_use]
    pub fn has_subscribers(&self) -> bool {
        self.sender.receiver_count() > 0
    }

    /// Listen for events from now on.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.sender.subscribe()
    }
}

/// What the daemon reports about every pane it is running.
#[must_use]
pub fn panes_of(session: &Session) -> Vec<PaneInfo> {
    session
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
            context_window: pane.context_window,
            exit: pane.exit,
        })
        .collect()
}

/// Subscribe `outbox`'s client, and send it the state of the session at once
/// so that it starts from something complete rather than from the next
/// change.
pub async fn start(session: &Arc<Session>, bus: &Bus, outbox: &Outbox) -> Response {
    // Subscribed before anything is read, so a change landing in between is
    // sent rather than lost.
    let events = bus.subscribe();
    let changes = session.changes();

    tokio::spawn(stream(Arc::clone(session), outbox.clone(), events, changes));
    Response::Ok
}

/// Push events to one client until it goes away.
async fn stream(
    session: Arc<Session>,
    outbox: Outbox,
    mut events: broadcast::Receiver<Event>,
    mut changes: tokio::sync::watch::Receiver<u64>,
) {
    let mut sent_panes: Option<Vec<PaneInfo>> = None;
    let mut sent_usage = None;

    // The first messages describe the session as it is, which is what a
    // client needs before it can make sense of a change.
    if !send_state(&session, &outbox, &mut sent_panes, &mut sent_usage).await {
        return;
    }

    loop {
        tokio::select! {
            () = outbox.closed() => return,

            changed = changes.changed() => {
                if changed.is_err() {
                    // The session is gone, which only happens while the
                    // daemon shuts down.
                    return;
                }
                if !send_state(&session, &outbox, &mut sent_panes, &mut sent_usage).await {
                    return;
                }
            }

            received = events.recv() => match received {
                Ok(event) => {
                    if !outbox.send(Response::Event { event }).await {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(missed)) => {
                    tracing::debug!(missed, "a subscribed client fell behind");
                    // Whatever it missed, the state below is the truth now.
                    sent_panes = None;
                    sent_usage = None;
                    if !send_state(&session, &outbox, &mut sent_panes, &mut sent_usage).await {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => return,
            },
        }
    }
}

/// Send what changed about the session itself, and remember what was sent.
///
/// Returns whether the client is still there.
async fn send_state(
    session: &Arc<Session>,
    outbox: &Outbox,
    sent_panes: &mut Option<Vec<PaneInfo>>,
    sent_usage: &mut Option<otag_features::usage::Report>,
) -> bool {
    let panes = panes_of(session);
    if sent_panes.as_ref() != Some(&panes) {
        if !outbox
            .send(Response::Event {
                event: Event::Panes {
                    panes: panes.clone(),
                },
            })
            .await
        {
            return false;
        }
        *sent_panes = Some(panes);
    }

    let usage = session.usage();
    if sent_usage.as_ref() != Some(&usage) {
        if !outbox
            .send(Response::Event {
                event: Event::Usage {
                    report: Box::new(usage.clone()),
                },
            })
            .await
        {
            return false;
        }
        *sent_usage = Some(usage);
    }

    true
}
