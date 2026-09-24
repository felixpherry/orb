//! orb's saved projects and threads, kept in SQLite across launches.
//!
//! For each project it keeps the directory orb started sessions in. For each
//! thread it keeps the Claude ids, the title, how far the transcript has been
//! read, and when the running turn started. The schema grows through an ordered
//! list of migrations. Times are milliseconds since the Unix epoch.

use std::path::{Path, PathBuf};

use error_stack::{Report, ResultExt};
use rusqlite::{Connection, Row, TransactionBehavior, params};
use wherror::Error;

use super::state::{ProjectId, ThreadId};

#[derive(Debug, Error)]
#[error(debug)]
pub struct StoreError;

/// A saved project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRow {
    pub id: ProjectId,
    /// The directory orb started the project's sessions in.
    pub root: PathBuf,
    pub title: String,
    pub created_at: i64,
}

/// A saved thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadRow {
    pub id: ThreadId,
    pub project_id: ProjectId,
    /// The id `claude --bg` printed; matches `agents --json` records.
    pub short_id: String,
    /// The Claude session id, once a poll has seen it.
    pub session_id: Option<String>,
    pub title: Option<String>,
    pub cwd: PathBuf,
    /// The transcript file, once it has been found.
    pub transcript_path: Option<PathBuf>,
    /// How many bytes of the transcript have been read.
    pub transcript_offset: u64,
    pub created_at: i64,
    /// When orb first saw the current turn running; `None` between turns.
    pub turn_started_at: Option<i64>,
}

/// A thread that was just created and isn't saved yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewThread {
    pub project_id: ProjectId,
    pub short_id: String,
    pub cwd: PathBuf,
    pub created_at: i64,
}

/// orb's database of projects and threads.
#[derive(Debug)]
pub struct Store {
    conn: Connection,
}

/// Schema migrations in order: entry `i` moves `user_version` from `i` to `i + 1`.
const MIGRATIONS: &[&str] = &["
    CREATE TABLE projects (
      id INTEGER PRIMARY KEY, root TEXT NOT NULL UNIQUE, title TEXT NOT NULL, created_at INTEGER NOT NULL);
    CREATE TABLE threads (
      id INTEGER PRIMARY KEY, project_id INTEGER NOT NULL REFERENCES projects(id),
      short_id TEXT NOT NULL UNIQUE, session_id TEXT, title TEXT, cwd TEXT NOT NULL,
      transcript_path TEXT, transcript_offset INTEGER NOT NULL DEFAULT 0,
      created_at INTEGER NOT NULL, turn_started_at INTEGER);
"];

impl Store {
    /// Opens the database at `path`, creating it and its directories if needed,
    /// and brings its schema up to date.
    ///
    /// # Errors
    ///
    /// Returns an error if the directories or the database can't be created,
    /// or a migration fails.
    pub fn open(path: &Path) -> Result<Self, Report<StoreError>> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .change_context(StoreError)
                .attach_with(|| format!("failed to create {}", parent.display()))?;
        }
        let conn = Connection::open(path)
            .change_context(StoreError)
            .attach_with(|| format!("failed to open {}", path.display()))?;
        Self::from_connection(conn)
    }

    /// Opens a fresh database that lives only in memory.
    ///
    /// # Errors
    ///
    /// Returns an error if SQLite can't open it or a migration fails.
    pub fn open_in_memory() -> Result<Self, Report<StoreError>> {
        let conn = Connection::open_in_memory()
            .change_context(StoreError)
            .attach("failed to open an in-memory database")?;
        Self::from_connection(conn)
    }

    fn from_connection(mut conn: Connection) -> Result<Self, Report<StoreError>> {
        conn.pragma_update(None, "foreign_keys", true)
            .change_context(StoreError)
            .attach("failed to enable foreign keys")?;
        migrate(&mut conn)?;
        Ok(Self { conn })
    }

    /// Every saved project (oldest first) and thread (newest first).
    ///
    /// # Errors
    ///
    /// Returns an error if the database can't be read.
    pub fn load(&self) -> Result<(Vec<ProjectRow>, Vec<ThreadRow>), Report<StoreError>> {
        let projects = self
            .query(
                "SELECT id, root, title, created_at FROM projects ORDER BY created_at, id",
                project_row,
            )
            .attach("failed to load projects")?;
        let threads = self
            .query(
                "SELECT id, project_id, short_id, session_id, title, cwd, transcript_path,
                        transcript_offset, created_at, turn_started_at
                 FROM threads ORDER BY created_at DESC, id DESC",
                thread_row,
            )
            .attach("failed to load threads")?;
        Ok((projects, threads))
    }

    /// Saves the project rooted at `root`, or retitles it if it exists.
    /// The same root always gets the same id.
    ///
    /// # Errors
    ///
    /// Returns an error if `root` isn't UTF-8 or the database can't be written.
    pub fn upsert_project(
        &self,
        root: &Path,
        title: &str,
        now_ms: i64,
    ) -> Result<ProjectId, Report<StoreError>> {
        self.conn
            .query_row(
                "INSERT INTO projects (root, title, created_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT (root) DO UPDATE SET title = excluded.title
                 RETURNING id",
                params![utf8(root)?, title, now_ms],
                |row| row.get(0),
            )
            .map(ProjectId)
            .change_context(StoreError)
            .attach("failed to save the project")
    }

    /// Saves a new thread.
    ///
    /// # Errors
    ///
    /// Returns an error if the cwd isn't UTF-8, the short id is already saved,
    /// the project doesn't exist, or the database can't be written.
    pub fn insert_thread(&self, row: &NewThread) -> Result<ThreadId, Report<StoreError>> {
        self.conn
            .query_row(
                "INSERT INTO threads (project_id, short_id, cwd, created_at)
                 VALUES (?1, ?2, ?3, ?4) RETURNING id",
                params![
                    row.project_id.0,
                    row.short_id,
                    utf8(&row.cwd)?,
                    row.created_at
                ],
                |row| row.get(0),
            )
            .map(ThreadId)
            .change_context(StoreError)
            .attach("failed to save the thread")
    }

    /// Updates what polling learns about a thread: its session id, title,
    /// transcript position, and turn start.
    ///
    /// # Errors
    ///
    /// Returns an error if the transcript path isn't UTF-8 or the database
    /// can't be written.
    pub fn save_thread(&self, row: &ThreadRow) -> Result<(), Report<StoreError>> {
        let transcript_path = row.transcript_path.as_deref().map(utf8).transpose()?;
        self.conn
            .execute(
                "UPDATE threads SET session_id = ?2, title = ?3, transcript_path = ?4,
                        transcript_offset = ?5, turn_started_at = ?6
                 WHERE id = ?1",
                params![
                    row.id.0,
                    row.session_id,
                    row.title,
                    transcript_path,
                    row.transcript_offset,
                    row.turn_started_at,
                ],
            )
            .change_context(StoreError)
            .attach("failed to update the thread")?;
        Ok(())
    }

    fn query<T, F>(&self, sql: &str, map: F) -> Result<Vec<T>, Report<StoreError>>
    where
        F: FnMut(&Row<'_>) -> rusqlite::Result<T>,
    {
        let mut statement = self.conn.prepare(sql).change_context(StoreError)?;
        let rows = statement
            .query_map([], map)
            .change_context(StoreError)?
            .collect::<rusqlite::Result<Vec<T>>>()
            .change_context(StoreError)?;
        Ok(rows)
    }
}

/// Applies the migrations the database hasn't seen, all in one transaction.
fn migrate(conn: &mut Connection) -> Result<(), Report<StoreError>> {
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .change_context(StoreError)
        .attach("failed to start the migration")?;
    let version: usize = tx
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .change_context(StoreError)
        .attach("failed to read the schema version")?;
    let pending = MIGRATIONS.get(version..).ok_or_else(|| {
        Report::new(StoreError).attach(format!(
            "the database's schema version {version} is newer than this orb knows"
        ))
    })?;
    if pending.is_empty() {
        return Ok(());
    }
    for sql in pending {
        tx.execute_batch(sql)
            .change_context(StoreError)
            .attach("failed to apply a migration")?;
    }
    tx.pragma_update(None, "user_version", MIGRATIONS.len())
        .change_context(StoreError)
        .attach("failed to record the schema version")?;
    tx.commit()
        .change_context(StoreError)
        .attach("failed to commit the migration")
}

fn utf8(path: &Path) -> Result<&str, Report<StoreError>> {
    path.to_str().ok_or_else(|| {
        Report::new(StoreError).attach(format!("the path {} isn't UTF-8", path.display()))
    })
}

fn project_row(row: &Row<'_>) -> rusqlite::Result<ProjectRow> {
    Ok(ProjectRow {
        id: ProjectId(row.get(0)?),
        root: PathBuf::from(row.get::<_, String>(1)?),
        title: row.get(2)?,
        created_at: row.get(3)?,
    })
}

fn thread_row(row: &Row<'_>) -> rusqlite::Result<ThreadRow> {
    Ok(ThreadRow {
        id: ThreadId(row.get(0)?),
        project_id: ProjectId(row.get(1)?),
        short_id: row.get(2)?,
        session_id: row.get(3)?,
        title: row.get(4)?,
        cwd: PathBuf::from(row.get::<_, String>(5)?),
        transcript_path: row.get::<_, Option<String>>(6)?.map(PathBuf::from),
        transcript_offset: row.get(7)?,
        created_at: row.get(8)?,
        turn_started_at: row.get(9)?,
    })
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "tests propagate store failures with `?` and assert on the outcome"
)]
mod tests {
    use std::path::{Path, PathBuf};

    use error_stack::{Report, ResultExt};
    use rusqlite::Connection;

    use super::{MIGRATIONS, NewThread, ProjectId, Store, StoreError, ThreadRow};

    fn user_version(path: &Path) -> Result<usize, Report<StoreError>> {
        Connection::open(path)
            .and_then(|conn| conn.pragma_query_value(None, "user_version", |row| row.get(0)))
            .change_context(StoreError)
    }

    fn new_thread(project_id: ProjectId) -> NewThread {
        NewThread {
            project_id,
            short_id: "28bf38e2".to_owned(),
            cwd: PathBuf::from("/tmp/orb"),
            created_at: 1_000,
        }
    }

    #[rstest::rstest]
    fn migrating_a_fresh_database_sets_the_latest_schema_version() -> Result<(), Report<StoreError>>
    {
        // Given a path with no database.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let path = dir.path().join("state.sqlite");

        // When opening the store.
        drop(Store::open(&path)?);

        // Then the database is at the latest schema version.
        assert_eq!(
            user_version(&path)?,
            MIGRATIONS.len(),
            "a fresh database should run every migration"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn reopening_a_migrated_database_keeps_its_schema_version() -> Result<(), Report<StoreError>> {
        // Given a database that was already migrated.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let path = dir.path().join("state.sqlite");
        drop(Store::open(&path)?);

        // When opening the store again.
        drop(Store::open(&path)?);

        // Then it opens and stays at the latest schema version.
        assert_eq!(
            user_version(&path)?,
            MIGRATIONS.len(),
            "migrating twice should be a no-op"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn inserted_thread_loads_back_after_reopening() -> Result<(), Report<StoreError>> {
        // Given a thread saved in a store that was then closed.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let path = dir.path().join("state.sqlite");
        let (project_id, thread_id) = {
            let store = Store::open(&path)?;
            let project_id = store.upsert_project(Path::new("/tmp/orb"), "orb", 500)?;
            (project_id, store.insert_thread(&new_thread(project_id))?)
        };

        // When reopening the store and loading.
        let (_, threads) = Store::open(&path)?.load()?;

        // Then the thread comes back with the fields it was saved with.
        let expected = ThreadRow {
            id: thread_id,
            project_id,
            short_id: "28bf38e2".to_owned(),
            session_id: None,
            title: None,
            cwd: PathBuf::from("/tmp/orb"),
            transcript_path: None,
            transcript_offset: 0,
            created_at: 1_000,
            turn_started_at: None,
        };
        assert_eq!(threads, vec![expected], "the saved thread should load back");
        Ok(())
    }

    #[rstest::rstest]
    fn upserting_the_same_root_twice_returns_the_same_id() -> Result<(), Report<StoreError>> {
        // Given a store with a project rooted at /tmp/orb.
        let store = Store::open_in_memory()?;
        let first = store.upsert_project(Path::new("/tmp/orb"), "orb", 500)?;

        // When upserting the same root again.
        let second = store.upsert_project(Path::new("/tmp/orb"), "orb", 900)?;

        // Then it gets the same id.
        assert_eq!(first, second, "one root should be one project");
        Ok(())
    }

    #[rstest::rstest]
    fn saved_thread_updates_load_back() -> Result<(), Report<StoreError>> {
        // Given a store with one thread.
        let store = Store::open_in_memory()?;
        let project_id = store.upsert_project(Path::new("/tmp/orb"), "orb", 500)?;
        let thread_id = store.insert_thread(&new_thread(project_id))?;

        // When saving its session id, title, transcript cursor, and turn start.
        let updated = ThreadRow {
            id: thread_id,
            project_id,
            short_id: "28bf38e2".to_owned(),
            session_id: Some("5f0c1c1e-session".to_owned()),
            title: Some("Fix the sidebar".to_owned()),
            cwd: PathBuf::from("/tmp/orb"),
            transcript_path: Some(PathBuf::from(
                "/tmp/claude/projects/-tmp-orb/5f0c1c1e.jsonl",
            )),
            transcript_offset: 4_096,
            created_at: 1_000,
            turn_started_at: Some(2_000),
        };
        store.save_thread(&updated)?;

        // Then loading returns the updated values.
        let (_, threads) = store.load()?;
        assert_eq!(threads, vec![updated], "the updates should load back");
        Ok(())
    }

    #[rstest::rstest]
    fn opening_under_a_missing_directory_creates_it() -> Result<(), Report<StoreError>> {
        // Given a database path whose parent directories don't exist.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let path = dir
            .path()
            .join("userdata")
            .join("nested")
            .join("state.sqlite");

        // When opening the store.
        drop(Store::open(&path)?);

        // Then the directories and the database file exist.
        assert!(path.is_file(), "opening should create {}", path.display());
        Ok(())
    }
}
