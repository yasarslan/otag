//! Keeping the daemon's session on disk, and putting it back.
//!
//! The daemon saves the shape of its session whenever it changes, and the
//! lines its panes have shown after screen changes, coalesced over ten
//! seconds. Nothing wakes or writes history while every pane is idle. Saving
//! happens on a thread that may block: SQLite is a file, not a service, and the
//! runtime serving the panes must never wait on a disk.
//!
//! Nothing here is allowed to take the daemon down. A database that cannot be
//! opened, read or written means otag runs without remembering anything,
//! which is worse than persistence working and better than no otag at all.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use otag_core::pane::PaneId;
use otag_core::session::Session;
use otag_core::snapshot::{self, Snapshot};
use otag_persist::Store;

/// How long after a change the daemon waits before saving.
///
/// A split is three changes in a row — a pane starting, the layout moving,
/// the panes being resized — and the user may follow it with another. Waiting
/// a moment turns a burst into one save.
const SETTLE: Duration = Duration::from_millis(250);

/// How often the lines every pane has shown are saved.
const HISTORY_INTERVAL: Duration = Duration::from_secs(10);

/// The daemon's snapshot database, usable from an async runtime.
#[derive(Debug, Clone)]
pub struct Snapshots {
    store: Arc<Mutex<Store>>,
    path: PathBuf,
}

impl Snapshots {
    /// Open the database, or report why it cannot be used.
    pub fn open(path: PathBuf) -> Result<Self, otag_persist::Error> {
        let store = Store::open(path.clone())?;
        Ok(Self {
            store: Arc::new(Mutex::new(store)),
            path,
        })
    }

    /// Where the database is.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// The saved session, if there is one that can be read.
    pub async fn load(&self) -> Option<Snapshot> {
        self.with(|store| store.load_session())
            .await
            .unwrap_or_else(|| {
                tracing::warn!("starting a fresh session");
                None
            })
    }

    /// The saved history of each of these panes.
    pub async fn history(&self, panes: Vec<PaneId>) -> HashMap<PaneId, Vec<String>> {
        self.with(move |store| {
            let mut lines = HashMap::with_capacity(panes.len());
            for pane in panes {
                lines.insert(pane, store.load_history(pane)?);
            }
            Ok(lines)
        })
        .await
        .unwrap_or_default()
    }

    /// Save the shape of the session.
    pub async fn save_session(&self, snapshot: Snapshot) {
        self.with(move |store| store.save_session(&snapshot)).await;
    }

    /// Save the lines these panes have shown, and forget the panes that are
    /// no longer there.
    pub async fn save_history(&self, histories: Vec<(PaneId, Vec<String>)>) {
        self.with(move |store| {
            let panes: Vec<PaneId> = histories.iter().map(|(pane, _)| *pane).collect();
            for (pane, lines) in &histories {
                store.save_history(*pane, lines)?;
            }
            store.keep_only(&panes)
        })
        .await;
    }

    /// Run one database call on a thread that may block, logging a failure
    /// rather than passing it on.
    async fn with<T, F>(&self, work: F) -> Option<T>
    where
        T: Send + 'static,
        F: FnOnce(&Store) -> otag_persist::Result<T> + Send + 'static,
    {
        let store = Arc::clone(&self.store);
        let done = tokio::task::spawn_blocking(move || {
            // A poisoned lock means an earlier call panicked inside SQLite.
            // Giving up on persistence is the right answer; taking the
            // daemon down with it is not.
            let store = store.lock().ok()?;
            match work(&store) {
                Ok(value) => Some(value),
                Err(error) => {
                    tracing::warn!(%error, "otag cannot use its state database");
                    None
                }
            }
        })
        .await;

        match done {
            Ok(value) => value,
            Err(error) => {
                tracing::warn!(%error, "a save of otag's state did not finish");
                None
            }
        }
    }
}

/// Put a saved session back, if there is one.
///
/// The session is left alone when there is nothing to restore, so a daemon
/// starting for the first time simply carries on.
pub async fn restore(session: &Arc<Session>, snapshots: &Snapshots) {
    let Some(snapshot) = snapshots.load().await else {
        return;
    };
    if snapshot.is_empty() || !snapshot.is_readable() {
        return;
    }

    let panes = snapshot.panes.iter().map(|pane| pane.id).collect();
    let history = snapshots.history(panes).await;

    let found = |pane: PaneId| history.get(&pane).cloned().unwrap_or_default();
    match session.restore(&snapshot, &found).await {
        Ok(()) => tracing::info!(
            workspaces = snapshot.workspaces.len(),
            panes = snapshot.panes.len(),
            "restored the saved otag session"
        ),
        Err(error) => tracing::warn!(%error, "cannot restore the saved otag session"),
    }
}

/// Keep saving the session for as long as the daemon runs.
///
/// Ends when the session is gone, which happens as the daemon shuts down.
pub async fn keep_saving(session: Arc<Session>, snapshots: Snapshots) {
    let mut changes = session.changes();
    let mut content = session.content_changes();
    let mut history_due =
        (!session.is_empty()).then(|| tokio::time::Instant::now() + HISTORY_INTERVAL);

    loop {
        tokio::select! {
            changed = changes.changed() => {
                if changed.is_err() {
                    return;
                }
                // Let a burst of changes finish before writing any of them.
                tokio::time::sleep(SETTLE).await;
                changes.mark_unchanged();
                snapshots.save_session(session.snapshot()).await;
            }
            changed = content.changed() => {
                if changed.is_err() { return; }
                history_due.get_or_insert_with(|| tokio::time::Instant::now() + HISTORY_INTERVAL);
            }
            () = async {
                match history_due {
                    Some(deadline) => tokio::time::sleep_until(deadline).await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                history_due = None;
                content.mark_unchanged();
                save_everything(&session, &snapshots).await;
            }
        }
    }
}

/// Put the usage history back after a restart.
///
/// A projection needs the samples of the last twenty minutes, so a daemon
/// that has just started would otherwise show percentages with no slope
/// behind them for as long as it takes to collect three more.
pub async fn restore_usage(shared: &crate::usage::Shared, snapshots: &Snapshots) {
    let samples = snapshots.with(|store| store.load_usage()).await;
    let Some(samples) = samples else {
        return;
    };
    if !samples.is_empty() {
        tracing::info!(samples = samples.len(), "usage history restored");
        shared.load(samples);
    }
}

/// Save the usage history.
pub async fn save_usage(shared: &crate::usage::Shared, snapshots: &Snapshots) {
    let samples = shared.samples();
    if samples.is_empty() {
        return;
    }
    snapshots
        .with(move |store| store.save_usage(&samples))
        .await;
}

/// Save the session and the lines every pane has shown.
pub async fn save_everything(session: &Session, snapshots: &Snapshots) {
    snapshots.save_session(session.snapshot()).await;
    snapshots
        .save_history(session.histories(snapshot::HISTORY_LINES).await)
        .await;
}
