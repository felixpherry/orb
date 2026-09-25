//! orb's saved projects and threads, kept in SQLite across launches.
//!
//! For each project it keeps the directory orb started sessions in. For each
//! thread it keeps the Claude ids, the titles, the git branch, how far the
//! transcript has been read, when the running turn started, whether it is
//! pinned or settled, and when it last had activity and was last visited. The
//! schema grows through an ordered list of migrations. Times are milliseconds
//! since the Unix epoch.

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
    /// Claude's latest generated title, else the first prompt.
    pub title: Option<String>,
    /// The latest title the user gave with `/rename`.
    pub custom_title: Option<String>,
    pub cwd: PathBuf,
    /// The transcript file, once it has been found.
    pub transcript_path: Option<PathBuf>,
    /// How many bytes of the transcript have been read.
    pub transcript_offset: u64,
    pub created_at: i64,
    /// When orb first saw the current turn running; `None` between turns.
    pub turn_started_at: Option<i64>,
    /// The git branch the transcript last named.
    pub branch: Option<String>,
    /// When the user pinned the thread; `None` = not pinned.
    pub pinned_at: Option<i64>,
    /// Whether the thread is settled or kept active; `None` = neither.
    pub settled_override: Option<SettledOverride>,
    /// When the thread was settled.
    pub settled_at: Option<i64>,
    /// When the thread was last un-settled.
    pub unsettled_at: Option<i64>,
    /// When orb last saw a turn end, else when the thread was created.
    pub last_activity_at: i64,
    /// When the user last selected the thread.
    pub last_visited_at: i64,
}

/// A thread's settle state as set by the user, auto-settle, or activity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettledOverride {
    /// Settled onto the shelf.
    Settled,
    /// Un-settled by the user; auto-settle waits for the next turn activity.
    Active,
}

impl SettledOverride {
    fn as_str(self) -> &'static str {
        match self {
            Self::Settled => "settled",
            Self::Active => "active",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "settled" => Some(Self::Settled),
            "active" => Some(Self::Active),
            _ => None,
        }
    }
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
const MIGRATIONS: &[&str] = &[
    "
    CREATE TABLE projects (
      id INTEGER PRIMARY KEY, root TEXT NOT NULL UNIQUE, title TEXT NOT NULL, created_at INTEGER NOT NULL);
    CREATE TABLE threads (
      id INTEGER PRIMARY KEY, project_id INTEGER NOT NULL REFERENCES projects(id),
      short_id TEXT NOT NULL UNIQUE, session_id TEXT, title TEXT, cwd TEXT NOT NULL,
      transcript_path TEXT, transcript_offset INTEGER NOT NULL DEFAULT 0,
      created_at INTEGER NOT NULL, turn_started_at INTEGER);
",
    "ALTER TABLE threads ADD COLUMN custom_title TEXT;",
    "
    ALTER TABLE threads ADD COLUMN branch TEXT;
    ALTER TABLE threads ADD COLUMN pinned_at INTEGER;
    ALTER TABLE threads ADD COLUMN settled_override TEXT;
    ALTER TABLE threads ADD COLUMN settled_at INTEGER;
    ALTER TABLE threads ADD COLUMN unsettled_at INTEGER;
    ALTER TABLE threads ADD COLUMN last_activity_at INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE threads ADD COLUMN last_visited_at INTEGER NOT NULL DEFAULT 0;
    UPDATE threads SET last_activity_at = created_at, last_visited_at = created_at;
",
];

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
                        transcript_offset, created_at, turn_started_at, custom_title,
                        branch, pinned_at, settled_override, settled_at, unsettled_at,
                        last_activity_at, last_visited_at
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

    /// Saves a new thread, last active and last visited when it was created.
    ///
    /// # Errors
    ///
    /// Returns an error if the cwd isn't UTF-8, the short id is already saved,
    /// the project doesn't exist, or the database can't be written.
    pub fn insert_thread(&self, row: &NewThread) -> Result<ThreadId, Report<StoreError>> {
        self.conn
            .query_row(
                "INSERT INTO threads
                   (project_id, short_id, cwd, created_at, last_activity_at, last_visited_at)
                 VALUES (?1, ?2, ?3, ?4, ?4, ?4) RETURNING id",
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

    /// Updates everything about a thread that changes after it's created: its
    /// session id, titles, branch, transcript position, turn start, pin and
    /// settle state, and activity and visit stamps.
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
                        transcript_offset = ?5, turn_started_at = ?6, custom_title = ?7,
                        branch = ?8, pinned_at = ?9, settled_override = ?10, settled_at = ?11,
                        unsettled_at = ?12, last_activity_at = ?13, last_visited_at = ?14
                 WHERE id = ?1",
                params![
                    row.id.0,
                    row.session_id,
                    row.title,
                    transcript_path,
                    row.transcript_offset,
                    row.turn_started_at,
                    row.custom_title,
                    row.branch,
                    row.pinned_at,
                    row.settled_override.map(SettledOverride::as_str),
                    row.settled_at,
                    row.unsettled_at,
                    row.last_activity_at,
                    row.last_visited_at,
                ],
            )
            .change_context(StoreError)
            .attach("failed to update the thread")?;
        Ok(())
    }

    /// Deletes a thread. Its project stays.
    ///
    /// # Errors
    ///
    /// Returns an error if the database can't be written.
    pub fn delete_thread(&self, id: ThreadId) -> Result<(), Report<StoreError>> {
        self.conn
            .execute("DELETE FROM threads WHERE id = ?1", params![id.0])
            .change_context(StoreError)
            .attach("failed to delete the thread")?;
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
        custom_title: row.get(10)?,
        cwd: PathBuf::from(row.get::<_, String>(5)?),
        transcript_path: row.get::<_, Option<String>>(6)?.map(PathBuf::from),
        transcript_offset: row.get(7)?,
        created_at: row.get(8)?,
        turn_started_at: row.get(9)?,
        branch: row.get(11)?,
        pinned_at: row.get(12)?,
        settled_override: row
            .get::<_, Option<String>>(13)?
            .as_deref()
            .and_then(SettledOverride::parse),
        settled_at: row.get(14)?,
        unsettled_at: row.get(15)?,
        last_activity_at: row.get(16)?,
        last_visited_at: row.get(17)?,
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

    use super::{MIGRATIONS, NewThread, ProjectId, SettledOverride, Store, StoreError, ThreadRow};

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
    fn migrating_a_v1_database_adds_the_custom_title_column() -> Result<(), Report<StoreError>> {
        // Given a database at schema version 1.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let path = dir.path().join("state.sqlite");
        {
            let v1 = MIGRATIONS
                .first()
                .ok_or_else(|| Report::new(StoreError).attach("no v1 migration"))?;
            let conn = Connection::open(&path).change_context(StoreError)?;
            conn.execute_batch(v1).change_context(StoreError)?;
            conn.pragma_update(None, "user_version", 1)
                .change_context(StoreError)?;
        }

        // When opening the store.
        drop(Store::open(&path)?);

        // Then it's at the latest version with a custom_title column.
        let has_column = Connection::open(&path)
            .change_context(StoreError)?
            .prepare("SELECT custom_title FROM threads")
            .is_ok();
        assert_eq!(
            (user_version(&path)?, has_column),
            (MIGRATIONS.len(), true),
            "migration v2 should add threads.custom_title"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn migrating_a_v2_database_seeds_activity_and_visit_from_created_at()
    -> Result<(), Report<StoreError>> {
        // Given a database at schema version 2 holding a thread created at 1 s.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let path = dir.path().join("state.sqlite");
        {
            let conn = Connection::open(&path).change_context(StoreError)?;
            for sql in MIGRATIONS
                .get(..2)
                .ok_or_else(|| Report::new(StoreError).attach("no v2 migrations"))?
            {
                conn.execute_batch(sql).change_context(StoreError)?;
            }
            conn.execute_batch(
                "INSERT INTO projects (id, root, title, created_at) VALUES (1, '/tmp/orb', 'orb', 500);
                 INSERT INTO threads (project_id, short_id, cwd, created_at)
                 VALUES (1, '28bf38e2', '/tmp/orb', 1000);
                 PRAGMA user_version = 2;",
            )
            .change_context(StoreError)?;
        }

        // When opening the store and loading.
        let (_, threads) = Store::open(&path)?.load()?;

        // Then it's at the latest version and both stamps are the creation time.
        let stamps: Vec<(i64, i64)> = threads
            .iter()
            .map(|row| (row.last_activity_at, row.last_visited_at))
            .collect();
        assert_eq!(
            (user_version(&path)?, stamps),
            (MIGRATIONS.len(), vec![(1_000, 1_000)]),
            "migration v3 should seed last activity and last visit from created_at"
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
            custom_title: None,
            cwd: PathBuf::from("/tmp/orb"),
            transcript_path: None,
            transcript_offset: 0,
            created_at: 1_000,
            turn_started_at: None,
            branch: None,
            pinned_at: None,
            settled_override: None,
            settled_at: None,
            unsettled_at: None,
            last_activity_at: 1_000,
            last_visited_at: 1_000,
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

        // When saving its session id, titles, transcript cursor, and turn start.
        let updated = ThreadRow {
            id: thread_id,
            project_id,
            short_id: "28bf38e2".to_owned(),
            session_id: Some("5f0c1c1e-session".to_owned()),
            title: Some("Fix the sidebar".to_owned()),
            custom_title: Some("sidebar".to_owned()),
            cwd: PathBuf::from("/tmp/orb"),
            transcript_path: Some(PathBuf::from(
                "/tmp/claude/projects/-tmp-orb/5f0c1c1e.jsonl",
            )),
            transcript_offset: 4_096,
            created_at: 1_000,
            turn_started_at: Some(2_000),
            branch: None,
            pinned_at: None,
            settled_override: None,
            settled_at: None,
            unsettled_at: None,
            last_activity_at: 1_000,
            last_visited_at: 1_000,
        };
        store.save_thread(&updated)?;

        // Then loading returns the updated values.
        let (_, threads) = store.load()?;
        assert_eq!(threads, vec![updated], "the updates should load back");
        Ok(())
    }

    #[rstest::rstest]
    #[case(SettledOverride::Settled)]
    #[case(SettledOverride::Active)]
    fn settle_fields_load_back_after_saving(
        #[case] settled_override: SettledOverride,
    ) -> Result<(), Report<StoreError>> {
        // Given a store with one thread.
        let store = Store::open_in_memory()?;
        let project_id = store.upsert_project(Path::new("/tmp/orb"), "orb", 500)?;
        store.insert_thread(&new_thread(project_id))?;
        let inserted = store
            .load()?
            .1
            .pop()
            .ok_or_else(|| Report::new(StoreError).attach("the thread wasn't saved"))?;

        // When saving its branch, pin, settle state, and stamps.
        let updated = ThreadRow {
            branch: Some("main".to_owned()),
            pinned_at: Some(2_000),
            settled_override: Some(settled_override),
            settled_at: Some(3_000),
            unsettled_at: Some(1_500),
            last_activity_at: 2_500,
            last_visited_at: 2_600,
            ..inserted
        };
        store.save_thread(&updated)?;

        // Then loading returns them.
        let (_, threads) = store.load()?;
        assert_eq!(threads, vec![updated], "the settle fields should load back");
        Ok(())
    }

    #[rstest::rstest]
    fn deleted_thread_is_gone_after_reload() -> Result<(), Report<StoreError>> {
        // Given a store with one thread.
        let store = Store::open_in_memory()?;
        let project_id = store.upsert_project(Path::new("/tmp/orb"), "orb", 500)?;
        let thread_id = store.insert_thread(&new_thread(project_id))?;

        // When deleting it.
        store.delete_thread(thread_id)?;

        // Then loading no longer returns it.
        let (_, threads) = store.load()?;
        assert!(threads.is_empty(), "the deleted thread should not load");
        Ok(())
    }

    #[rstest::rstest]
    fn inserted_thread_starts_seen_and_active_at_creation() -> Result<(), Report<StoreError>> {
        // Given a store with a project.
        let store = Store::open_in_memory()?;
        let project_id = store.upsert_project(Path::new("/tmp/orb"), "orb", 500)?;

        // When inserting a thread created at 1 s.
        store.insert_thread(&new_thread(project_id))?;

        // Then its last activity and last visit are its creation time.
        let (_, threads) = store.load()?;
        let stamps: Vec<(i64, i64)> = threads
            .iter()
            .map(|row| (row.last_activity_at, row.last_visited_at))
            .collect();
        assert_eq!(
            stamps,
            vec![(1_000, 1_000)],
            "a new thread should start seen and active at its creation"
        );
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
