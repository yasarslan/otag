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
use otag_features::usage::{self, Sample, Window};
use rusqlite::{Connection, OptionalExtension, params};

/// Name of the database file inside otag's state directory.
pub const DATABASE_FILE_NAME: &str = "state.sqlite";

/// The name a window is stored under.
///
/// Spelled out here rather than taken from the enum, so that renaming a
/// variant cannot silently orphan a week of samples.
const fn window_name(window: Window) -> &'static str {
    match window {
        Window::FiveHour => "five_hour",
        Window::Weekly => "weekly",
    }
}

/// The window one of those names means, if this build knows it.
fn window_from_name(name: &str) -> Option<Window> {
    Window::ALL
        .into_iter()
        .find(|window| window_name(*window) == name)
}

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
             );
             CREATE TABLE IF NOT EXISTS usage_samples (
                 agent     TEXT    NOT NULL,
                 window    TEXT    NOT NULL,
                 at        INTEGER NOT NULL,
                 used_pct  REAL    NOT NULL,
                 resets_at INTEGER,
                 session   TEXT,
                 PRIMARY KEY (agent, window, at)
             );",
        )?;
        self.add_missing_columns();
        Ok(())
    }

    /// Bring a database an older otag created up to today's columns.
    ///
    /// A column that is already there makes `ALTER TABLE` fail, and that
    /// failure is the answer rather than a problem: the point is only that
    /// the column exists afterwards. Nothing here can lose a reading, so
    /// none of it is worth refusing to start over.
    fn add_missing_columns(&self) {
        let statement = "ALTER TABLE usage_samples ADD COLUMN session TEXT;";
        if self.connection.execute_batch(statement).is_err() {
            tracing::debug!(statement, "the database already has this column");
        }
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

    /// Save usage samples, and forget what is older than a week.
    ///
    /// A sample otag already has is left alone: the same reading arriving
    /// twice is one reading, and the key says so.
    pub fn save_usage(&self, samples: &[Sample]) -> Result<()> {
        if samples.is_empty() {
            return Ok(());
        }
        let newest = samples.iter().map(|sample| sample.at).max().unwrap_or(0);

        let mut statement = self
            .connection
            .prepare(
                "INSERT INTO usage_samples (agent, window, at, used_pct, resets_at, session)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT (agent, window, at) DO UPDATE
                 SET used_pct = ?4, resets_at = ?5, session = ?6",
            )
            .map_err(|source| self.failed(source))?;
        for sample in samples {
            statement
                .execute(params![
                    sample.agent.name(),
                    window_name(sample.window),
                    sample.at,
                    f64::from(sample.used_pct),
                    sample.resets_at,
                    sample.session.as_deref(),
                ])
                .map_err(|source| self.failed(source))?;
        }
        drop(statement);

        self.connection
            .execute(
                "DELETE FROM usage_samples WHERE at < ?1",
                params![newest.saturating_sub(usage::HISTORY)],
            )
            .map_err(|source| self.failed(source))?;
        Ok(())
    }

    /// Every usage sample that was saved, oldest first.
    ///
    /// A row whose agent or window this build does not know is skipped: it
    /// was written by a newer otag, and inventing a meaning for it would be
    /// worse than one missing reading.
    pub fn load_usage(&self) -> Result<Vec<Sample>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT agent, window, at, used_pct, resets_at, session
                 FROM usage_samples ORDER BY at",
            )
            .map_err(|source| self.failed(source))?;

        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, f64>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            })
            .map_err(|source| self.failed(source))?;

        let mut samples = Vec::new();
        for row in rows {
            let (agent, window, at, used_pct, resets_at, session) =
                row.map_err(|source| self.failed(source))?;
            let (Ok(agent), Some(window)) = (agent.parse(), window_from_name(&window)) else {
                continue;
            };
            samples.push(Sample {
                agent,
                window,
                used_pct: used_pct as f32,
                resets_at,
                at,
                session,
            });
        }
        Ok(samples)
    }

    /// Forget the whole saved session.
    pub fn clear(&self) -> Result<()> {
        self.execute("DELETE FROM session; DELETE FROM history;")
    }

    /// Forget every usage sample.
    pub fn clear_usage(&self) -> Result<()> {
        self.execute("DELETE FROM usage_samples;")
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
            agent_models: std::collections::BTreeMap::new(),
            teams: Vec::new(),
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
                    nickname: String::new(),
                    cwd: Some(PathBuf::from("/projects/app")),
                    command: Vec::new(),
                },
                PaneSnapshot {
                    id: pane(2),
                    title: "claude".to_owned(),
                    nickname: String::new(),
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

    #[test]
    fn usage_samples_survive_a_round_trip() {
        let store = Store::open_in_memory().expect("a database");
        let samples = [
            Sample {
                agent: otag_agents::AgentKind::Claude,
                window: Window::FiveHour,
                used_pct: 62.5,
                resets_at: Some(1_700_000_000),
                at: 1_699_999_000,
                session: Some("a9f87cc5-726a".to_owned()),
            },
            Sample {
                agent: otag_agents::AgentKind::Codex,
                window: Window::Weekly,
                used_pct: 3.0,
                resets_at: None,
                at: 1_699_999_500,
                session: None,
            },
        ];

        store.save_usage(&samples).expect("the samples are saved");
        let read = store.load_usage().expect("and read back");

        assert_eq!(read.len(), 2);
        assert_eq!(read[0].used_pct, 62.5);
        assert_eq!(read[0].resets_at, Some(1_700_000_000));
        assert_eq!(read[1].agent, otag_agents::AgentKind::Codex);
        assert_eq!(read[1].window, Window::Weekly);
        assert_eq!(read[1].resets_at, None);
        assert_eq!(
            read[0].session.as_deref(),
            Some("a9f87cc5-726a"),
            "which session reported a reading survives a restart"
        );
        assert_eq!(read[1].session, None);
    }

    #[test]
    fn a_database_file_an_older_otag_wrote_is_opened_and_read() {
        // The path the user's own database takes: a file written by a build
        // that had no `session` column, opened by one that has.
        let directory = tempfile::tempdir().expect("a temporary directory");
        let path = directory.path().join(DATABASE_FILE_NAME);
        {
            let old = Connection::open(&path).expect("a database file");
            old.execute_batch(
                "CREATE TABLE usage_samples (
                     agent     TEXT    NOT NULL,
                     window    TEXT    NOT NULL,
                     at        INTEGER NOT NULL,
                     used_pct  REAL    NOT NULL,
                     resets_at INTEGER,
                     PRIMARY KEY (agent, window, at)
                 );
                 INSERT INTO usage_samples
                 VALUES ('claude', 'five_hour', 1700000000, 25.0, 1700007200);",
            )
            .expect("the older schema");
        }

        let store = Store::open(&path).expect("the file opens");
        let read = store.load_usage().expect("and its readings are read");
        assert_eq!(read.len(), 1, "a reading written before the column is kept");
        assert_eq!(read[0].used_pct, 25.0);
        assert_eq!(
            read[0].resets_at,
            Some(1_700_007_200),
            "including when its window resets, which is what makes it current"
        );
        assert_eq!(read[0].session, None);

        // And the file is writable from then on, with the new column.
        store
            .save_usage(&[Sample {
                agent: otag_agents::AgentKind::Claude,
                window: Window::FiveHour,
                used_pct: 26.0,
                resets_at: Some(1_700_007_200),
                at: 1_700_000_100,
                session: Some("sess-1".to_owned()),
            }])
            .expect("saved");
        let read = store.load_usage().expect("read back");
        assert_eq!(read.len(), 2);
        assert_eq!(read[1].session.as_deref(), Some("sess-1"));
    }

    #[test]
    fn a_database_from_an_older_otag_gains_the_columns_it_is_missing() {
        let store = Store::open_in_memory().expect("a database");
        // The table as an older otag wrote it, with a reading already in it.
        store
            .execute(
                "DROP TABLE usage_samples;
                 CREATE TABLE usage_samples (
                     agent     TEXT    NOT NULL,
                     window    TEXT    NOT NULL,
                     at        INTEGER NOT NULL,
                     used_pct  REAL    NOT NULL,
                     resets_at INTEGER,
                     PRIMARY KEY (agent, window, at)
                 );
                 INSERT INTO usage_samples VALUES ('claude', 'five_hour', 1700000000, 42.0, NULL);",
            )
            .expect("an older table");

        store.add_missing_columns();

        let read = store
            .load_usage()
            .expect("the old readings are still there");
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].used_pct, 42.0);
        assert_eq!(read[0].session, None, "an old row never said which session");

        // And the new column works from then on.
        store
            .save_usage(&[Sample {
                agent: otag_agents::AgentKind::Claude,
                window: Window::FiveHour,
                used_pct: 43.0,
                resets_at: None,
                at: 1_700_000_100,
                session: Some("later".to_owned()),
            }])
            .expect("saved");
        let read = store.load_usage().expect("read back");
        assert_eq!(read[1].session.as_deref(), Some("later"));
    }

    #[test]
    fn the_same_reading_saved_twice_is_one_row() {
        let store = Store::open_in_memory().expect("a database");
        let sample = Sample {
            agent: otag_agents::AgentKind::Claude,
            window: Window::FiveHour,
            used_pct: 10.0,
            resets_at: None,
            at: 1_700_000_000,
            session: None,
        };

        store
            .save_usage(std::slice::from_ref(&sample))
            .expect("saved");
        store
            .save_usage(&[Sample {
                used_pct: 11.0,
                ..sample
            }])
            .expect("saved again");

        let read = store.load_usage().expect("read back");
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].used_pct, 11.0, "the newer reading wins");
    }

    #[test]
    fn usage_samples_older_than_a_week_are_dropped_on_save() {
        let store = Store::open_in_memory().expect("a database");
        let old = Sample {
            agent: otag_agents::AgentKind::Claude,
            window: Window::FiveHour,
            used_pct: 1.0,
            resets_at: None,
            at: 1_000_000,
            session: None,
        };
        store.save_usage(std::slice::from_ref(&old)).expect("saved");
        store
            .save_usage(&[Sample {
                at: 1_000_000 + usage::HISTORY + 1,
                ..old
            }])
            .expect("saved");

        let read = store.load_usage().expect("read back");
        assert_eq!(read.len(), 1, "a week of samples is enough");
    }

    #[test]
    fn a_sample_written_by_a_newer_otag_is_skipped_rather_than_guessed_at() {
        let store = Store::open_in_memory().expect("a database");
        store
            .connection
            .execute(
                "INSERT INTO usage_samples (agent, window, at, used_pct, resets_at)
                 VALUES ('gemini', 'five_hour', 1, 5.0, NULL),
                        ('claude', 'fortnight', 2, 5.0, NULL)",
                [],
            )
            .expect("rows from the future");

        assert!(store.load_usage().expect("read back").is_empty());
    }

    #[test]
    fn saving_no_samples_does_nothing() {
        let store = Store::open_in_memory().expect("a database");
        store.save_usage(&[]).expect("nothing to do");
        assert!(store.load_usage().expect("read back").is_empty());
    }

    #[test]
    fn usage_samples_can_be_forgotten() {
        let store = Store::open_in_memory().expect("a database");
        store
            .save_usage(&[Sample {
                agent: otag_agents::AgentKind::Claude,
                window: Window::FiveHour,
                used_pct: 1.0,
                resets_at: None,
                at: 1,
                session: None,
            }])
            .expect("saved");
        store.clear_usage().expect("forgotten");
        assert!(store.load_usage().expect("read back").is_empty());
    }
}
