//! Persistence for otag: state snapshots to SQLite, and restore of
//! workspaces, layout and history after a daemon restart.
//!
//! The whole structure of a session — workspaces, tabs, layouts, what each
//! pane was running — is kept as one JSON document, because it is small,
//! changes as a whole, and must survive being read by a different version of
//! otag than the one that wrote it. History is kept per pane and
//! compressed, because it is large, changes on its own schedule, and is
//! worth nothing to anything but the pane it belongs to.
//!
//! Every call here blocks: SQLite is a file, not a service. Callers on an
//! async runtime must put these on a thread that may block.
#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

use otag_core::pane::PaneId;
use otag_core::snapshot::Snapshot;
use rusqlite::{Connection, OptionalExtension, params};

/// Name of the database file inside otag's state directory.
pub const DATABASE_FILE_NAME: &str = "state.sqlite";

/// How hard a pane's saved lines are compressed.
///
/// Low: the lines are saved every few seconds and read once, so the time
/// matters more than the last few percent of size.
const COMPRESSION_LEVEL: i32 = 3;

/// Everything that can go wrong while saving or restoring.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The database could not be opened or used.
    #[error("cannot use the otag state database {path}: {source}")]
    Database {
        /// Path of the database.
        path: PathBuf,
        /// What SQLite reported.
        #[source]
        source: rusqlite::Error,
    },

    /// A snapshot could not be encoded or decoded.
    #[error("the saved otag session could not be read: {0}")]
    Snapshot(#[from] serde_json::Error),
}

/// Convenience alias for results produced by this crate.
pub type Result<T> = std::result::Result<T, Error>;

/// otag's snapshot database.
#[derive(Debug)]
pub struct Store {
    connection: Connection,
    path: PathBuf,
}

impl Store {
    /// Open the database at `path`, creating it and its directory if needed.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent()
            && let Err(error) = std::fs::create_dir_all(parent)
        {
            tracing::warn!(%error, directory = %parent.display(), "cannot create otag's state directory");
        }

        let connection = Connection::open(&path).map_err(|source| Error::Database {
            path: path.clone(),
            source,
        })?;
        let store = Self { connection, path };
        store.prepare()?;
        Ok(store)
    }

    /// Open a database that lives only as long as the process, for tests.
    pub fn open_in_memory() -> Result<Self> {
        let connection = Connection::open_in_memory().map_err(|source| Error::Database {
            path: PathBuf::from(":memory:"),
            source,
        })?;
        let store = Self {
            connection,
            path: PathBuf::from(":memory:"),
        };
        store.prepare()?;
        Ok(store)
    }

    /// Where the database is.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Create the tables, and choose the settings a daemon wants.
    fn prepare(&self) -> Result<()> {
        // `WAL` keeps a save from blocking anything else that reads, and
        // `NORMAL` means a save does not wait for the disk to confirm:
        // losing the last few seconds of a snapshot after a power cut is a
        // far smaller problem than a daemon that stutters every ten seconds.
        self.execute(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             CREATE TABLE IF NOT EXISTS session (
                 id       INTEGER PRIMARY KEY CHECK (id = 1),
                 saved_at INTEGER NOT NULL,
                 snapshot TEXT    NOT NULL
             );
             CREATE TABLE IF NOT EXISTS history (
                 pane     INTEGER PRIMARY KEY,
                 saved_at INTEGER NOT NULL,
                 lines    BLOB    NOT NULL
             );",
        )
    }

    /// Save the structure of a session, replacing what was there.
    pub fn save_session(&self, snapshot: &Snapshot) -> Result<()> {
        let encoded = serde_json::to_string(snapshot)?;
        self.connection
            .execute(
                "INSERT INTO session (id, saved_at, snapshot) VALUES (1, ?1, ?2)
                 ON CONFLICT (id) DO UPDATE SET saved_at = ?1, snapshot = ?2",
                params![now(), encoded],
            )
            .map_err(|source| self.failed(source))?;
        Ok(())
    }

    /// The saved session, if there is one that can be read.
    ///
    /// A row that cannot be decoded is reported as an error rather than
    /// quietly dropped, so that a daemon can say what happened; the caller
    /// then starts a fresh session.
    pub fn load_session(&self) -> Result<Option<Snapshot>> {
        let encoded: Option<String> = self
            .connection
            .query_row("SELECT snapshot FROM session WHERE id = 1", [], |row| {
                row.get(0)
            })
            .optional()
            .map_err(|source| self.failed(source))?;

        match encoded {
            Some(encoded) => Ok(Some(serde_json::from_str(&encoded)?)),
            None => Ok(None),
        }
    }

    /// Save one pane's scrolled-off lines, replacing what was there.
    pub fn save_history(&self, pane: PaneId, lines: &[String]) -> Result<()> {
        let compressed = compress(lines);
        self.connection
            .execute(
                "INSERT INTO history (pane, saved_at, lines) VALUES (?1, ?2, ?3)
                 ON CONFLICT (pane) DO UPDATE SET saved_at = ?2, lines = ?3",
                params![pane.get(), now(), compressed],
            )
            .map_err(|source| self.failed(source))?;
        Ok(())
    }

    /// One pane's saved lines, oldest first, empty when there are none.
    pub fn load_history(&self, pane: PaneId) -> Result<Vec<String>> {
        let compressed: Option<Vec<u8>> = self
            .connection
            .query_row(
                "SELECT lines FROM history WHERE pane = ?1",
                params![pane.get()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|source| self.failed(source))?;

        match compressed {
            Some(compressed) => decompress(&compressed),
            None => Ok(Vec::new()),
        }
    }

    /// Forget the history of every pane but these.
    ///
    /// Panes are never reused, so without this the database would grow for
    /// as long as otag is installed.
    pub fn keep_only(&self, panes: &[PaneId]) -> Result<()> {
        let kept = panes
            .iter()
            .map(|pane| pane.get().to_string())
            .collect::<Vec<_>>()
            .join(",");
        // The list is built from numbers otag generated, so there is nothing
        // in it a value could be smuggled through.
        self.execute(&format!("DELETE FROM history WHERE pane NOT IN ({kept})"))
    }

    /// Forget the whole saved session.
    pub fn clear(&self) -> Result<()> {
        self.execute("DELETE FROM session; DELETE FROM history;")
    }

    fn execute(&self, sql: &str) -> Result<()> {
        self.connection
            .execute_batch(sql)
            .map_err(|source| self.failed(source))
    }

    fn failed(&self, source: rusqlite::Error) -> Error {
        Error::Database {
            path: self.path.clone(),
            source,
        }
    }
}

/// Seconds since the epoch, or zero on a clock that cannot be read.
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| i64::try_from(since.as_secs()).unwrap_or(0))
}

/// Pack lines into one compressed blob.
///
/// Compression that fails is not worth an error: the lines are stored as
/// they are, which the other side can tell apart because it decompresses
/// only what decompresses.
fn compress(lines: &[String]) -> Vec<u8> {
    let joined = lines.join("\n");
    match zstd::encode_all(joined.as_bytes(), COMPRESSION_LEVEL) {
        Ok(compressed) => compressed,
        Err(error) => {
            tracing::debug!(%error, "keeping a pane's history uncompressed");
            joined.into_bytes()
        }
    }
}

/// Unpack what [`compress`] produced.
fn decompress(blob: &[u8]) -> Result<Vec<String>> {
    let bytes = match zstd::decode_all(blob) {
        Ok(bytes) => bytes,
        // Not a compressed blob, so it is the plain text fallback.
        Err(_) => blob.to_vec(),
    };
    let text = String::from_utf8_lossy(&bytes);
    if text.is_empty() {
        return Ok(Vec::new());
    }
    Ok(text.split('\n').map(str::to_owned).collect())
}

#[cfg(test)]
mod tests {
    use otag_core::layout::{Direction, Layout};
    use otag_core::pane::PaneSize;
    use otag_core::snapshot::{PaneSnapshot, TabSnapshot, WorkspaceSnapshot};
    use otag_core::workspace::{TabId, WorkspaceId};

    use super::*;

    fn pane(number: u64) -> PaneId {
        PaneId::from_number(number)
    }

    fn snapshot() -> Snapshot {
        let mut layout = Layout::new(pane(1));
        assert!(layout.split(pane(1), Direction::TopBottom, pane(2)));

        Snapshot {
            version: otag_core::snapshot::VERSION,
            size: PaneSize::new(30, 100),
            sidebar: true,
            active_workspace: 0,
            workspaces: vec![WorkspaceSnapshot {
                id: WorkspaceId::FIRST,
                name: "app".to_owned(),
                cwd: Some(PathBuf::from("/projects/app")),
                active_tab: 0,
                tabs: vec![TabSnapshot {
                    id: TabId::FIRST,
                    title: "shell".to_owned(),
                    focus: pane(2),
                    layout,
                }],
            }],
            panes: vec![
                PaneSnapshot {
                    id: pane(1),
                    title: "shell".to_owned(),
                    cwd: Some(PathBuf::from("/projects/app")),
                    command: Vec::new(),
                },
                PaneSnapshot {
                    id: pane(2),
                    title: "claude".to_owned(),
                    cwd: Some(PathBuf::from("/projects/app")),
                    command: vec!["claude".to_owned(), "--resume".to_owned()],
                },
            ],
        }
    }

    #[test]
    fn a_session_comes_back_as_it_was_saved() {
        let store = Store::open_in_memory().expect("a database");
        assert_eq!(store.load_session().expect("nothing saved yet"), None);

        let snapshot = snapshot();
        store.save_session(&snapshot).expect("the session saves");
        assert_eq!(store.load_session().expect("it loads"), Some(snapshot));
    }

    #[test]
    fn saving_again_replaces_what_was_there() {
        let store = Store::open_in_memory().expect("a database");
        store.save_session(&snapshot()).expect("the first save");

        let second = Snapshot {
            active_workspace: 0,
            workspaces: Vec::new(),
            panes: Vec::new(),
            ..snapshot()
        };
        store.save_session(&second).expect("the second save");
        assert_eq!(store.load_session().expect("it loads"), Some(second));
    }

    #[test]
    fn history_comes_back_line_for_line() {
        let store = Store::open_in_memory().expect("a database");
        assert!(
            store
                .load_history(pane(1))
                .expect("nothing saved yet")
                .is_empty()
        );

        let lines: Vec<String> = (1..=1000).map(|line| format!("line {line}")).collect();
        store
            .save_history(pane(1), &lines)
            .expect("the history saves");
        assert_eq!(store.load_history(pane(1)).expect("it loads"), lines);
        assert!(
            store
                .load_history(pane(2))
                .expect("another pane has none")
                .is_empty()
        );
    }

    #[test]
    fn a_pane_with_no_history_saves_and_loads_as_nothing() {
        let store = Store::open_in_memory().expect("a database");
        store.save_history(pane(1), &[]).expect("an empty save");
        assert!(store.load_history(pane(1)).expect("it loads").is_empty());
    }

    #[test]
    fn lines_with_text_from_any_language_survive() {
        let store = Store::open_in_memory().expect("a database");
        let lines = vec![
            "a line with ümlauts and accents".to_owned(),
            "\u{4e2d}\u{6587}\u{3068}\u{65e5}\u{672c}\u{8a9e}".to_owned(),
            "\u{1f600} emoji".to_owned(),
        ];
        store.save_history(pane(3), &lines).expect("it saves");
        assert_eq!(store.load_history(pane(3)).expect("it loads"), lines);
    }

    #[test]
    fn the_history_is_compressed() {
        // A thousand similar lines must not take a thousand lines' worth of
        // space: a busy pane is saved every few seconds.
        let lines: Vec<String> = (0..1000).map(|_| "a repeated line".to_owned()).collect();
        let plain = lines.join("\n").len();
        assert!(
            compress(&lines).len() * 10 < plain,
            "compression did not help"
        );
    }

    #[test]
    fn the_history_of_panes_that_are_gone_is_forgotten() {
        let store = Store::open_in_memory().expect("a database");
        for number in 1..=3 {
            store
                .save_history(pane(number), &["something".to_owned()])
                .expect("it saves");
        }

        store.keep_only(&[pane(2)]).expect("the others are dropped");
        assert!(store.load_history(pane(1)).expect("it loads").is_empty());
        assert!(!store.load_history(pane(2)).expect("it loads").is_empty());
        assert!(store.load_history(pane(3)).expect("it loads").is_empty());

        // Keeping nothing is a sound request, not a broken query.
        store.keep_only(&[]).expect("everything is dropped");
        assert!(store.load_history(pane(2)).expect("it loads").is_empty());
    }

    #[test]
    fn clearing_leaves_nothing_to_restore() {
        let store = Store::open_in_memory().expect("a database");
        store.save_session(&snapshot()).expect("it saves");
        store
            .save_history(pane(1), &["something".to_owned()])
            .expect("it saves");

        store.clear().expect("it clears");
        assert_eq!(store.load_session().expect("it loads"), None);
        assert!(store.load_history(pane(1)).expect("it loads").is_empty());
    }

    #[test]
    fn a_database_survives_being_closed_and_opened_again() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let path = directory.path().join("state").join(DATABASE_FILE_NAME);

        {
            let store = Store::open(&path).expect("a database");
            store.save_session(&snapshot()).expect("it saves");
            store
                .save_history(pane(1), &["kept".to_owned()])
                .expect("it saves");
        }

        let store = Store::open(&path).expect("the database opens again");
        assert_eq!(store.load_session().expect("it loads"), Some(snapshot()));
        assert_eq!(
            store.load_history(pane(1)).expect("it loads"),
            vec!["kept".to_owned()]
        );
    }

    #[test]
    fn a_session_saved_by_a_build_that_knew_less_still_loads() {
        let store = Store::open_in_memory().expect("a database");
        store
            .connection
            .execute(
                "INSERT INTO session (id, saved_at, snapshot) VALUES (1, 0, ?1)",
                params![r#"{"version":1}"#],
            )
            .expect("the row is written");

        let loaded = store.load_session().expect("it loads").expect("a snapshot");
        assert!(loaded.is_empty());
        assert!(loaded.is_readable());
    }

    #[test]
    fn a_session_that_cannot_be_read_is_reported_rather_than_guessed_at() {
        let store = Store::open_in_memory().expect("a database");
        store
            .connection
            .execute(
                "INSERT INTO session (id, saved_at, snapshot) VALUES (1, 0, ?1)",
                params!["not json at all"],
            )
            .expect("the row is written");

        let error = store.load_session().expect_err("it cannot be read");
        assert!(error.to_string().contains("could not be read"), "{error}");
    }

    #[test]
    fn a_path_that_cannot_be_opened_says_which_one() {
        let error = Store::open("/no/such/directory/anywhere/state.sqlite")
            .expect_err("the database cannot be opened");
        assert!(error.to_string().contains("state.sqlite"), "{error}");
    }
}
