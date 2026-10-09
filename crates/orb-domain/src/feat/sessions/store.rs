//! orb's saved projects, threads and sessions, kept in SQLite across
//! launches.
//!
//! For each project it keeps the directory its sessions start in. For each
//! thread it keeps the session ids, the titles, the git branch, how far the
//! transcript has been read, when the running turn started, whether it is
//! pinned or settled, when it last had activity and was last visited, and the
//! model and permission mode its session started with. It also keeps the
//! sidebar's width and project filter. For each project it also keeps its
//! kind (one the user added, or orb's Research or Learn folder). It also
//! keeps the jump list's rows, oldest first. Each session
//! keeps its directory and its tabs in order, each tab its split tree,
//! focused pane, swap layout and whether it was changed by hand, and each
//! pane its directory, zmx session, name and the command that brings its
//! agent back; a thread keeps the pane it runs in.
//! The schema grows through an ordered list of migrations, and a database
//! from before sessions is copied aside before it is migrated. Times are
//! milliseconds since the Unix epoch.
//!
//! One orb at a time owns a store: it holds a lock beside the database for as
//! long as it runs, since a second orb on the same store would stop the
//! first one's panes.

use std::collections::HashMap;
use std::fs::{File, TryLockError};
use std::path::{Path, PathBuf};

use error_stack::{Report, ResultExt};
use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, params};
use wherror::Error;

use super::state::{PaneId, ProjectId, ProjectKind, SessionId, SessionKind, SidebarItem, ThreadId};
use crate::feat::harness::HarnessId;
use crate::feat::layout::tree::{Node, SwapLayout, TileLayout};

#[derive(Debug, Error)]
#[error(debug)]
pub struct StoreError;

/// A saved project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRow {
    pub id: ProjectId,
    /// The directory the project's sessions start in.
    pub root: PathBuf,
    pub title: String,
    pub created_at: i64,
    /// When the project was removed; `None` = not removed.
    pub removed_at: Option<i64>,
    pub kind: ProjectKind,
}

/// A saved thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadRow {
    pub id: ThreadId,
    pub project_id: ProjectId,
    /// The id the harness gave the session when it started it.
    pub short_id: String,
    /// The harness's session id, once a poll has seen it.
    pub session_id: Option<String>,
    /// The harness's latest generated title, else the first prompt.
    pub title: Option<String>,
    /// The latest title the user gave with `/rename`.
    pub custom_title: Option<String>,
    /// The name the user gave with `r`; beats every other title.
    pub renamed_title: Option<String>,
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
    /// When orb last saw a turn end, else when the thread was created.
    pub last_activity_at: i64,
    /// When the user last selected the thread.
    pub last_visited_at: i64,
    /// Whether the harness has generated a title for the thread.
    pub ai_titled: bool,
    /// The `--model` a session from before the overhaul started with;
    /// `None` = unknown.
    pub model: Option<String>,
    /// The harness its session runs in.
    pub harness: HarnessId,
    /// The pane it runs in; `None` once it has ended.
    pub pane_id: Option<PaneId>,
    /// The session it runs in, or ran in before it ended; `None` until it
    /// first runs in a pane.
    pub orb_session: Option<SessionId>,
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

/// A thread an agent report started in an existing pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPaneThread {
    pub pane: PaneId,
    /// The harness's id for the conversation; also its short id.
    pub session_id: String,
    pub transcript_path: Option<PathBuf>,
    pub harness: HarnessId,
    pub cwd: PathBuf,
    pub created_at: i64,
}

/// A saved session: what its layout needs, and its settle lifecycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRow {
    pub id: SessionId,
    pub project_id: ProjectId,
    pub kind: SessionKind,
    /// The name the user gave with `r`.
    pub name: Option<String>,
    /// The branch it was last recorded on.
    pub branch: Option<String>,
    pub created_at: i64,
    /// The directory its panes start in.
    pub dir: PathBuf,
    /// The position of the tab it shows.
    pub active_tab: usize,
    pub pinned_at: Option<i64>,
    pub settled_override: Option<SettledOverride>,
    pub settled_at: Option<i64>,
    pub unsettled_at: Option<i64>,
    /// When a turn last ended in one of its agent panes.
    pub last_activity_at: i64,
}

/// A saved tab of a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabRow {
    pub session_id: SessionId,
    /// Where the tab sits in the tab bar, from 0.
    pub position: usize,
    /// The name the user gave it.
    pub name: Option<String>,
    /// Its split tree, as `TileLayout::to_json` writes it.
    pub layout: String,
    /// Its focused pane.
    pub focus_pane: Option<PaneId>,
    /// Where it is in zellij's swap layout list.
    pub swap_layout: SwapLayout,
    /// Whether it was changed by hand since its last relayout.
    pub hand_changed: bool,
}

/// A saved pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneRow {
    pub id: PaneId,
    pub session_id: SessionId,
    /// The directory its program starts in.
    pub cwd: PathBuf,
    /// Its zmx session's name; `None` = `orb-p<id>`.
    pub zmx_name: Option<String>,
    /// Its zmx socket dir, relative to orb's folder unless absolute; `None`
    /// = orb's own pane dir.
    pub zmx_dir: Option<PathBuf>,
    /// The command to type into a fresh shell to bring its agent back.
    pub resume: Option<String>,
    /// A Claude `--bg` session the migration replaced, still to stop.
    pub migrated_bg: Option<String>,
    /// The name the user gave it.
    pub name: Option<String>,
}

/// Every saved session, tab and pane, for restoring layouts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SavedLayouts {
    pub sessions: Vec<SessionRow>,
    /// Ordered by session, then position.
    pub tabs: Vec<TabRow>,
    pub panes: Vec<PaneRow>,
}

/// How the user last left orb's layout; `None` = never set.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Ui {
    /// The sidebar's width in columns.
    pub sidebar_width: Option<u16>,
    /// The project the sidebar is filtered to.
    pub project_filter: Option<ProjectId>,
}

/// Everything [`Store::load`] returns: projects and threads.
pub type Saved = (Vec<ProjectRow>, Vec<ThreadRow>);

/// orb's database of projects, threads and sessions.
#[derive(Debug)]
pub struct Store {
    conn: Connection,
}

/// One schema step: SQL, or Rust for what SQL can't do (layout JSON).
#[derive(Clone, Copy)]
enum Migration {
    Sql(&'static str),
    Code(fn(&Connection) -> rusqlite::Result<()>),
}

impl Migration {
    fn apply(self, conn: &Connection) -> rusqlite::Result<()> {
        match self {
            Self::Sql(sql) => conn.execute_batch(sql),
            Self::Code(step) => step(conn),
        }
    }
}

/// Schema migrations in order: entry `i` moves `user_version` from `i` to `i + 1`.
const MIGRATIONS: &[Migration] = &[
    Migration::Sql("
    CREATE TABLE projects (
      id INTEGER PRIMARY KEY, root TEXT NOT NULL UNIQUE, title TEXT NOT NULL, created_at INTEGER NOT NULL);
    CREATE TABLE threads (
      id INTEGER PRIMARY KEY, project_id INTEGER NOT NULL REFERENCES projects(id),
      short_id TEXT NOT NULL UNIQUE, session_id TEXT, title TEXT, cwd TEXT NOT NULL,
      transcript_path TEXT, transcript_offset INTEGER NOT NULL DEFAULT 0,
      created_at INTEGER NOT NULL, turn_started_at INTEGER);
"),
    Migration::Sql("ALTER TABLE threads ADD COLUMN custom_title TEXT;"),
    Migration::Sql("
    ALTER TABLE threads ADD COLUMN branch TEXT;
    ALTER TABLE threads ADD COLUMN pinned_at INTEGER;
    ALTER TABLE threads ADD COLUMN settled_override TEXT;
    ALTER TABLE threads ADD COLUMN settled_at INTEGER;
    ALTER TABLE threads ADD COLUMN unsettled_at INTEGER;
    ALTER TABLE threads ADD COLUMN last_activity_at INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE threads ADD COLUMN last_visited_at INTEGER NOT NULL DEFAULT 0;
    UPDATE threads SET last_activity_at = created_at, last_visited_at = created_at;
"),
    Migration::Sql("ALTER TABLE threads ADD COLUMN ai_titled INTEGER NOT NULL DEFAULT 0;"),
    Migration::Sql("
    CREATE TABLE drafts (
      project_id INTEGER PRIMARY KEY REFERENCES projects(id),
      workspace TEXT NOT NULL, workspace_path TEXT,
      branch TEXT, model TEXT, permission_mode TEXT,
      created_at INTEGER NOT NULL);
    ALTER TABLE projects ADD COLUMN last_workspace TEXT;
    ALTER TABLE projects ADD COLUMN last_model TEXT;
    ALTER TABLE projects ADD COLUMN last_permission_mode TEXT;
    ALTER TABLE projects ADD COLUMN last_used_at INTEGER;
    ALTER TABLE threads ADD COLUMN model TEXT;
    ALTER TABLE threads ADD COLUMN permission_mode TEXT;
"),
    Migration::Sql("
    ALTER TABLE projects ADD COLUMN removed_at INTEGER;
    CREATE TABLE ui (
      id INTEGER PRIMARY KEY CHECK (id = 1),
      sidebar_width INTEGER,
      project_filter INTEGER REFERENCES projects(id));
"),
    Migration::Sql("ALTER TABLE threads ADD COLUMN renamed_title TEXT;"),
    Migration::Sql("
    ALTER TABLE projects ADD COLUMN kind TEXT;
    CREATE TABLE groups (
      id INTEGER PRIMARY KEY, project_id INTEGER NOT NULL REFERENCES projects(id),
      kind TEXT NOT NULL, name TEXT NOT NULL, dir TEXT, branch TEXT,
      created_at INTEGER NOT NULL,
      pinned_at INTEGER, settled_override TEXT, settled_at INTEGER, unsettled_at INTEGER,
      draft_model TEXT, draft_permission_mode TEXT,
      UNIQUE (project_id, kind, name));
    ALTER TABLE threads ADD COLUMN group_id INTEGER REFERENCES groups(id);
"),
    Migration::Sql("CREATE TABLE jumps (position INTEGER PRIMARY KEY, kind TEXT NOT NULL, item_id INTEGER NOT NULL);"),
    Migration::Sql("
    ALTER TABLE threads ADD COLUMN harness TEXT NOT NULL DEFAULT 'claude';
    ALTER TABLE drafts ADD COLUMN harness TEXT NOT NULL DEFAULT 'claude';
    ALTER TABLE projects ADD COLUMN last_harness TEXT;
    ALTER TABLE groups ADD COLUMN harness TEXT NOT NULL DEFAULT 'claude';
    ALTER TABLE groups ADD COLUMN has_draft INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE groups ADD COLUMN own_harness TEXT;
    ALTER TABLE groups ADD COLUMN own_model_set INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE groups ADD COLUMN own_model TEXT;
    ALTER TABLE groups ADD COLUMN own_permission_set INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE groups ADD COLUMN own_permission TEXT;
    UPDATE groups SET has_draft = 1
     WHERE id NOT IN (SELECT group_id FROM threads WHERE group_id IS NOT NULL);
"),
    Migration::Code(v11),
    Migration::Sql("
    ALTER TABLE tabs ADD COLUMN swap_layout TEXT NOT NULL DEFAULT 'vertical';
    ALTER TABLE tabs ADD COLUMN hand_changed INTEGER NOT NULL DEFAULT 0;
"),
];

/// Takes the store at `path` for this process: an exclusive lock on the
/// `.lock` file beside it, held until the returned file is dropped (or the
/// process ends).
///
/// # Errors
///
/// Returns an error if another process holds the lock, or the lock file
/// can't be created.
pub fn lock(path: &Path) -> Result<File, Report<StoreError>> {
    let lock_path = path.with_extension("lock");
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)
            .change_context(StoreError)
            .attach_with(|| format!("failed to create {}", parent.display()))?;
    }
    let file = File::create(&lock_path)
        .change_context(StoreError)
        .attach_with(|| format!("failed to create {}", lock_path.display()))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => Err(Report::new(StoreError)
            .attach("another orb is already running; quit it before starting this one")),
        Err(TryLockError::Error(error)) => Err(Report::new(error)
            .change_context(StoreError)
            .attach(format!("failed to lock {}", lock_path.display()))),
    }
}

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
        back_up_before_sessions(&conn, path)?;
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
    pub fn load(&self) -> Result<Saved, Report<StoreError>> {
        let projects = self
            .query(
                "SELECT id, root, title, created_at, removed_at, kind
                 FROM projects ORDER BY created_at, id",
                project_row,
            )
            .attach("failed to load projects")?;
        let threads = self
            .query(
                "SELECT id, project_id, short_id, session_id, title, cwd, transcript_path,
                        transcript_offset, created_at, turn_started_at, custom_title,
                        branch, last_activity_at, last_visited_at, ai_titled, model,
                        renamed_title, harness, pane_id, orb_session_id
                 FROM threads ORDER BY created_at DESC, id DESC",
                thread_row,
            )
            .attach("failed to load threads")?;
        Ok((projects, threads))
    }

    /// Saves the project rooted at `root` unless one is already saved there,
    /// which keeps its title and creation time and is no longer removed. An
    /// existing root takes a Research or Learn kind, but adding it as Normal
    /// keeps the kind it had. The same root always gets the same id.
    ///
    /// # Errors
    ///
    /// Returns an error if `root` isn't UTF-8 or the database can't be written.
    pub fn add_project(
        &self,
        root: &Path,
        title: &str,
        kind: ProjectKind,
        now_ms: i64,
    ) -> Result<ProjectId, Report<StoreError>> {
        self.conn
            .query_row(
                "INSERT INTO projects (root, title, created_at, kind) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (root) DO UPDATE SET
                   removed_at = NULL, kind = COALESCE(excluded.kind, kind)
                 RETURNING id",
                params![utf8(root)?, title, now_ms, project_kind_text(kind)],
                |row| row.get(0),
            )
            .map(ProjectId)
            .change_context(StoreError)
            .attach("failed to save the project")
    }

    /// Saves a new `kind` session of `project` named `name` in `dir` on
    /// `branch`, last active and visited at `now`, with one tab holding one shell pane.
    /// Returns the session and its pane.
    ///
    /// # Errors
    ///
    /// Returns an error if `dir` isn't UTF-8, the project doesn't exist, or the
    /// database can't be written.
    pub fn insert_session(
        &self,
        project: ProjectId,
        kind: SessionKind,
        dir: &Path,
        branch: Option<&str>,
        name: Option<&str>,
        now: i64,
    ) -> Result<(SessionId, PaneId), Report<StoreError>> {
        let dir = utf8(dir)?.to_owned();
        let tx = self
            .conn
            .unchecked_transaction()
            .change_context(StoreError)
            .attach("failed to start saving the session")?;
        let session = NewSession {
            project_id: project.0,
            kind: session_kind_text(kind).to_owned(),
            dir: dir.clone(),
            name: name.map(str::to_owned),
            branch: branch.map(str::to_owned),
            created_at: now,
            pinned_at: None,
            settled_override: None,
            settled_at: None,
            unsettled_at: None,
            last_activity_at: now,
            last_visited_at: now,
        };
        let (session, panes) = insert_session(&tx, &session, &[NewPane::shell(dir)])
            .change_context(StoreError)
            .attach("failed to save the session")?;
        let pane = panes
            .first()
            .copied()
            .ok_or_else(|| Report::new(StoreError).attach("the session has no pane"))?;
        tx.commit()
            .change_context(StoreError)
            .attach("failed to commit the session")?;
        Ok((session, pane))
    }

    /// Every saved session, its tabs in order, and every pane.
    ///
    /// # Errors
    ///
    /// Returns an error if the database can't be read.
    pub fn layouts(&self) -> Result<SavedLayouts, Report<StoreError>> {
        let sessions = self
            .query(
                "SELECT id, dir, active_tab, pinned_at, settled_override, settled_at,
                        unsettled_at, last_activity_at, project_id, kind, name, branch,
                        created_at
                 FROM sessions ORDER BY id",
                |row| {
                    Ok(SessionRow {
                        id: SessionId(row.get(0)?),
                        dir: PathBuf::from(row.get::<_, String>(1)?),
                        active_tab: row.get::<_, Option<usize>>(2)?.unwrap_or(0),
                        pinned_at: row.get(3)?,
                        settled_override: row
                            .get::<_, Option<String>>(4)?
                            .as_deref()
                            .and_then(SettledOverride::parse),
                        settled_at: row.get(5)?,
                        unsettled_at: row.get(6)?,
                        last_activity_at: row.get(7)?,
                        project_id: ProjectId(row.get(8)?),
                        kind: session_kind(&row.get::<_, String>(9)?),
                        name: row.get(10)?,
                        branch: row.get(11)?,
                        created_at: row.get(12)?,
                    })
                },
            )
            .attach("failed to load sessions")?;
        let tabs = self
            .query(
                "SELECT session_id, position, name, layout, focus_pane, swap_layout,
                        hand_changed
                 FROM tabs ORDER BY session_id, position",
                |row| {
                    Ok(TabRow {
                        session_id: SessionId(row.get(0)?),
                        position: row.get(1)?,
                        name: row.get(2)?,
                        layout: row.get(3)?,
                        focus_pane: row.get::<_, Option<i64>>(4)?.map(PaneId),
                        swap_layout: swap_layout(&row.get::<_, String>(5)?),
                        hand_changed: row.get(6)?,
                    })
                },
            )
            .attach("failed to load tabs")?;
        let panes = self
            .query(
                "SELECT id, session_id, cwd, zmx_name, zmx_dir, resume, migrated_bg, name
                 FROM panes ORDER BY id",
                |row| {
                    Ok(PaneRow {
                        id: PaneId(row.get(0)?),
                        session_id: SessionId(row.get(1)?),
                        cwd: PathBuf::from(row.get::<_, String>(2)?),
                        zmx_name: row.get(3)?,
                        zmx_dir: row.get::<_, Option<String>>(4)?.map(PathBuf::from),
                        resume: row.get(5)?,
                        migrated_bg: row.get(6)?,
                        name: row.get(7)?,
                    })
                },
            )
            .attach("failed to load panes")?;
        Ok(SavedLayouts {
            sessions,
            tabs,
            panes,
        })
    }

    /// Saves session `row`'s name, branch, pin, settle and activity state.
    ///
    /// # Errors
    ///
    /// Returns an error if the database can't be written.
    pub fn save_session(&self, row: &SessionRow) -> Result<(), Report<StoreError>> {
        self.conn
            .execute(
                "UPDATE sessions SET pinned_at = ?2, settled_override = ?3, settled_at = ?4,
                        unsettled_at = ?5, last_activity_at = ?6, name = ?7, branch = ?8
                 WHERE id = ?1",
                params![
                    row.id.0,
                    row.pinned_at,
                    row.settled_override.map(SettledOverride::as_str),
                    row.settled_at,
                    row.unsettled_at,
                    row.last_activity_at,
                    row.name,
                    row.branch,
                ],
            )
            .change_context(StoreError)
            .attach("failed to save the session")?;
        Ok(())
    }

    /// Moves `session` to `dir` on `branch`: its row's directory and branch,
    /// and every one of its panes' directories.
    ///
    /// # Errors
    ///
    /// Returns an error if `dir` isn't UTF-8 or the database can't be written.
    pub fn move_session(
        &self,
        session: SessionId,
        dir: &Path,
        branch: Option<&str>,
    ) -> Result<(), Report<StoreError>> {
        let dir = utf8(dir)?;
        let tx = self
            .conn
            .unchecked_transaction()
            .change_context(StoreError)
            .attach("failed to start moving the session")?;
        tx.execute(
            "UPDATE sessions SET dir = ?2, branch = ?3 WHERE id = ?1",
            params![session.0, dir, branch],
        )
        .and_then(|_| {
            tx.execute(
                "UPDATE panes SET cwd = ?2 WHERE session_id = ?1",
                params![session.0, dir],
            )
        })
        .change_context(StoreError)
        .attach("failed to move the session")?;
        tx.commit()
            .change_context(StoreError)
            .attach("failed to commit the session's move")
    }

    /// Saves a new pane running a shell in `cwd` in `session`.
    ///
    /// # Errors
    ///
    /// Returns an error if `cwd` isn't UTF-8, the session doesn't exist, or
    /// the database can't be written.
    pub fn insert_pane(
        &self,
        session: SessionId,
        cwd: &Path,
    ) -> Result<PaneId, Report<StoreError>> {
        self.conn
            .query_row(
                "INSERT INTO panes (session_id, cwd) VALUES (?1, ?2) RETURNING id",
                params![session.0, utf8(cwd)?],
                |row| row.get(0),
            )
            .map(PaneId)
            .change_context(StoreError)
            .attach("failed to save the pane")
    }

    /// Saves `session`'s tabs, replacing the ones it had, the position of its
    /// active tab, and the names of `panes`, which are every pane its tabs
    /// hold. Its other panes are deleted, and a thread running in one of them
    /// no longer has a pane. Returns the deleted panes.
    ///
    /// # Errors
    ///
    /// Returns an error if the session doesn't exist or the database can't be
    /// written.
    pub fn save_layout(
        &self,
        session: SessionId,
        active_tab: usize,
        tabs: &[TabRow],
        panes: &[(PaneId, Option<String>)],
    ) -> Result<Vec<PaneId>, Report<StoreError>> {
        let tx = self
            .conn
            .unchecked_transaction()
            .change_context(StoreError)
            .attach("failed to start saving the layout")?;
        let dropped = (|| {
            tx.execute("DELETE FROM tabs WHERE session_id = ?1", [session.0])?;
            for tab in tabs {
                tx.execute(
                    "INSERT INTO tabs (session_id, position, name, layout, focus_pane,
                                       swap_layout, hand_changed)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        session.0,
                        tab.position,
                        tab.name,
                        tab.layout,
                        tab.focus_pane.map(|pane| pane.0),
                        swap_layout_text(tab.swap_layout),
                        tab.hand_changed
                    ],
                )?;
            }
            tx.execute(
                "UPDATE sessions SET active_tab = ?2 WHERE id = ?1",
                params![session.0, active_tab],
            )?;
            for (pane, name) in panes {
                tx.execute(
                    "UPDATE panes SET name = ?3 WHERE id = ?1 AND session_id = ?2",
                    params![pane.0, session.0, name],
                )?;
            }
            let held: Vec<PaneId> = panes.iter().map(|(pane, _)| *pane).collect();
            let dropped: Vec<PaneId> = collect(
                &tx,
                "SELECT id FROM panes WHERE session_id = ?1 ORDER BY id",
                [session.0],
                |row| row.get(0).map(PaneId),
            )?
            .into_iter()
            .filter(|pane| !held.contains(pane))
            .collect();
            for pane in &dropped {
                tx.execute(
                    "UPDATE threads SET pane_id = NULL WHERE pane_id = ?1",
                    [pane.0],
                )?;
                tx.execute("DELETE FROM panes WHERE id = ?1", [pane.0])?;
            }
            Ok::<_, rusqlite::Error>(dropped)
        })()
        .change_context(StoreError)
        .attach("failed to save the layout")?;
        tx.commit()
            .change_context(StoreError)
            .attach("failed to commit the layout")?;
        Ok(dropped)
    }

    /// Deletes `session` with its tabs and panes, and the threads that run
    /// or ran in it.
    ///
    /// # Errors
    ///
    /// Returns an error if the database can't be written.
    pub fn delete_session(&self, session: SessionId) -> Result<(), Report<StoreError>> {
        let tx = self
            .conn
            .unchecked_transaction()
            .change_context(StoreError)
            .attach("failed to start deleting the session")?;
        [
            "DELETE FROM threads
              WHERE orb_session_id = ?1
                 OR pane_id IN (SELECT id FROM panes WHERE session_id = ?1)",
            "DELETE FROM tabs WHERE session_id = ?1",
            "DELETE FROM panes WHERE session_id = ?1",
            "DELETE FROM sessions WHERE id = ?1",
        ]
        .into_iter()
        .try_for_each(|sql| tx.execute(sql, [session.0]).map(drop))
        .change_context(StoreError)
        .attach("failed to delete the session")?;
        tx.commit()
            .change_context(StoreError)
            .attach("failed to commit deleting the session")
    }

    /// Saves a thread running in `row.pane`, in the project of that pane's
    /// session, last active and visited when created. Returns it and its
    /// project.
    ///
    /// # Errors
    ///
    /// Returns an error if a path isn't UTF-8, the pane doesn't exist, the id
    /// is already a thread's short id, or the database can't be written.
    pub fn insert_pane_thread(
        &self,
        row: &NewPaneThread,
    ) -> Result<(ThreadId, ProjectId), Report<StoreError>> {
        let transcript_path = row.transcript_path.as_deref().map(utf8).transpose()?;
        self.conn
            .query_row(
                "INSERT INTO threads
                   (project_id, short_id, session_id, cwd, transcript_path, created_at,
                    last_activity_at, last_visited_at, harness, pane_id, orb_session_id)
                 SELECT s.project_id, ?2, ?2, ?3, ?4, ?5, ?5, ?5, ?6, p.id, s.id
                   FROM panes p JOIN sessions s ON s.id = p.session_id
                  WHERE p.id = ?1
                 RETURNING id, project_id",
                params![
                    row.pane.0,
                    row.session_id,
                    utf8(&row.cwd)?,
                    transcript_path,
                    row.created_at,
                    row.harness.as_str(),
                ],
                |found| Ok((ThreadId(found.get(0)?), ProjectId(found.get(1)?))),
            )
            .change_context(StoreError)
            .attach("failed to save the pane's thread")
    }

    /// Sets the command typed into pane `pane`'s fresh shell; `None` = none.
    ///
    /// # Errors
    ///
    /// Returns an error if the database can't be written.
    pub fn set_pane_resume(
        &self,
        pane: PaneId,
        resume: Option<&str>,
    ) -> Result<(), Report<StoreError>> {
        self.conn
            .execute(
                "UPDATE panes SET resume = ?2 WHERE id = ?1",
                params![pane.0, resume],
            )
            .change_context(StoreError)
            .attach("failed to save the pane's resume command")?;
        Ok(())
    }

    /// Forgets the `--bg` sessions `panes` replaced, once they're stopped.
    ///
    /// # Errors
    ///
    /// Returns an error if the database can't be written.
    pub fn clear_migrated_bg(&self, panes: &[PaneId]) -> Result<(), Report<StoreError>> {
        for pane in panes {
            self.conn
                .execute(
                    "UPDATE panes SET migrated_bg = NULL WHERE id = ?1",
                    [pane.0],
                )
                .change_context(StoreError)
                .attach("failed to clear a pane's --bg session")?;
        }
        Ok(())
    }

    /// Updates everything about a thread that changes after it's created: its
    /// session and directory (a thread can move to another workspace), titles,
    /// branch, transcript position, turn start, activity and visit stamps,
    /// and the pane it runs in. Its model stays as inserted.
    ///
    /// # Errors
    ///
    /// Returns an error if the cwd or transcript path isn't UTF-8, the short
    /// id belongs to another thread, or the database can't be written.
    pub fn save_thread(&self, row: &ThreadRow) -> Result<(), Report<StoreError>> {
        let transcript_path = row.transcript_path.as_deref().map(utf8).transpose()?;
        self.conn
            .execute(
                "UPDATE threads SET session_id = ?2, title = ?3, transcript_path = ?4,
                        transcript_offset = ?5, turn_started_at = ?6, custom_title = ?7,
                        branch = ?8, last_activity_at = ?9, last_visited_at = ?10,
                        short_id = ?11, cwd = ?12, ai_titled = ?13, renamed_title = ?14,
                        pane_id = ?15, orb_session_id = ?16
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
                    row.last_activity_at,
                    row.last_visited_at,
                    row.short_id,
                    utf8(&row.cwd)?,
                    row.ai_titled,
                    row.renamed_title,
                    row.pane_id.map(|pane| pane.0),
                    row.orb_session.map(|session| session.0),
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

    /// Marks the project removed at `now_ms`. Its sessions stay.
    ///
    /// # Errors
    ///
    /// Returns an error if the database can't be written.
    pub fn remove_project(
        &self,
        project: ProjectId,
        now_ms: i64,
    ) -> Result<(), Report<StoreError>> {
        self.conn
            .execute(
                "UPDATE projects SET removed_at = ?2 WHERE id = ?1",
                params![project.0, now_ms],
            )
            .change_context(StoreError)
            .attach("failed to mark the project removed")?;
        Ok(())
    }

    /// How the user last left orb's layout; defaults if never saved.
    ///
    /// # Errors
    ///
    /// Returns an error if the database can't be read.
    pub fn ui(&self) -> Result<Ui, Report<StoreError>> {
        self.conn
            .query_row(
                "SELECT sidebar_width, project_filter FROM ui WHERE id = 1",
                [],
                ui_row,
            )
            .optional()
            .map(Option::unwrap_or_default)
            .change_context(StoreError)
            .attach("failed to read the layout settings")
    }

    /// Saves how the user left orb's layout, replacing what was saved.
    ///
    /// # Errors
    ///
    /// Returns an error if the filtered project doesn't exist or the database
    /// can't be written.
    pub fn save_ui(&self, ui: &Ui) -> Result<(), Report<StoreError>> {
        self.conn
            .execute(
                "INSERT INTO ui (id, sidebar_width, project_filter) VALUES (1, ?1, ?2)
                 ON CONFLICT (id) DO UPDATE SET sidebar_width = excluded.sidebar_width,
                                                project_filter = excluded.project_filter",
                params![ui.sidebar_width, ui.project_filter.map(|project| project.0)],
            )
            .change_context(StoreError)
            .attach("failed to save the layout settings")?;
        Ok(())
    }

    /// The saved jump list's rows, oldest first; a row of an unknown kind is
    /// skipped.
    ///
    /// # Errors
    ///
    /// Returns an error if the database can't be read.
    pub fn jumps(&self) -> Result<Vec<SidebarItem>, Report<StoreError>> {
        let rows = self
            .query("SELECT kind, item_id FROM jumps ORDER BY position", |row| {
                Ok(jump_item(&row.get::<_, String>(0)?, row.get(1)?))
            })
            .attach("failed to read the jump list")?;
        Ok(rows.into_iter().flatten().collect())
    }

    /// Saves the jump list's rows, oldest first, replacing what was saved.
    /// The Settled header is never saved.
    ///
    /// # Errors
    ///
    /// Returns an error if the database can't be written.
    pub fn save_jumps(&self, items: &[SidebarItem]) -> Result<(), Report<StoreError>> {
        let tx = self
            .conn
            .unchecked_transaction()
            .change_context(StoreError)
            .attach("failed to start saving the jump list")?;
        tx.execute("DELETE FROM jumps", [])
            .change_context(StoreError)
            .attach("failed to clear the saved jump list")?;
        for (position, (kind, id)) in items.iter().filter_map(|&item| jump_kind(item)).enumerate() {
            tx.execute(
                "INSERT INTO jumps (position, kind, item_id) VALUES (?1, ?2, ?3)",
                params![position, kind, id],
            )
            .change_context(StoreError)
            .attach("failed to save a jump")?;
        }
        tx.commit()
            .change_context(StoreError)
            .attach("failed to commit the jump list")
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
    for migration in pending {
        migration
            .apply(&tx)
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

/// The schema version that turned threads and groups into sessions.
const SESSIONS_VERSION: usize = 11;

/// Copies a database that predates sessions to `<file>.bak-v<version>` beside
/// it, once: an existing backup is kept. The copy is written to a `.tmp` file
/// with `VACUUM INTO` (a consistent snapshot, `user_version` included) and
/// renamed into place, so a half-written copy never counts as the backup. A
/// fresh database (version 0) or one already past the sessions migration
/// needs none.
fn back_up_before_sessions(conn: &Connection, path: &Path) -> Result<(), Report<StoreError>> {
    let version: usize = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .change_context(StoreError)
        .attach("failed to read the schema version")?;
    if version == 0 || version >= SESSIONS_VERSION {
        return Ok(());
    }
    let backup = {
        let mut name = path.as_os_str().to_owned();
        name.push(format!(".bak-v{version}"));
        PathBuf::from(name)
    };
    if backup.exists() {
        return Ok(());
    }
    let partial = {
        let mut name = backup.as_os_str().to_owned();
        name.push(".tmp");
        PathBuf::from(name)
    };
    if partial.exists() {
        std::fs::remove_file(&partial)
            .change_context(StoreError)
            .attach_with(|| format!("failed to remove {}", partial.display()))?;
    }
    conn.execute("VACUUM INTO ?1", params![utf8(&partial)?])
        .change_context(StoreError)
        .attach_with(|| format!("failed to back the database up to {}", partial.display()))?;
    std::fs::rename(&partial, &backup)
        .change_context(StoreError)
        .attach_with(|| format!("failed to move the backup to {}", backup.display()))
}

/// Turns every group into a session with a tab for each of its threads,
/// newest first, and every thread outside a group (or in a group that is
/// gone) into a session with one tab. Each thread gets a pane of its own,
/// alone in its tab. Thread ids don't change. Pane
/// ids start above every thread id, since earlier versions named zmx sessions
/// `orb-p<thread id>`. The `groups` and `drafts` tables and the threads' old
/// columns stay.
fn v11(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE sessions (
          id INTEGER PRIMARY KEY, project_id INTEGER NOT NULL REFERENCES projects(id),
          kind TEXT NOT NULL DEFAULT 'plain', dir TEXT NOT NULL, name TEXT, branch TEXT,
          created_at INTEGER NOT NULL, pinned_at INTEGER, settled_override TEXT,
          settled_at INTEGER, unsettled_at INTEGER,
          last_activity_at INTEGER NOT NULL DEFAULT 0, last_visited_at INTEGER NOT NULL DEFAULT 0,
          active_tab INTEGER);
        CREATE TABLE tabs (
          id INTEGER PRIMARY KEY, session_id INTEGER NOT NULL REFERENCES sessions(id),
          position INTEGER NOT NULL, name TEXT, layout TEXT NOT NULL, focus_pane INTEGER);
        CREATE TABLE panes (
          id INTEGER PRIMARY KEY AUTOINCREMENT,
          session_id INTEGER NOT NULL REFERENCES sessions(id),
          cwd TEXT NOT NULL, zmx_name TEXT, zmx_dir TEXT, resume TEXT, migrated_bg TEXT,
          name TEXT);
        ALTER TABLE threads ADD COLUMN pane_id INTEGER REFERENCES panes(id);
        ALTER TABLE threads ADD COLUMN orb_session_id INTEGER REFERENCES sessions(id);
        INSERT INTO sqlite_sequence (name, seq)
        VALUES ('panes', (SELECT COALESCE(MAX(id), 0) FROM threads));
        ",
    )?;
    let mut group_sessions = HashMap::new();
    let groups = collect(
        conn,
        "SELECT g.id, g.project_id, g.kind, COALESCE(g.dir, p.root), g.name, g.branch,
                g.created_at, g.pinned_at, g.settled_override, g.settled_at, g.unsettled_at,
                COALESCE((SELECT MAX(last_activity_at) FROM threads WHERE group_id = g.id),
                         g.created_at),
                COALESCE((SELECT MAX(last_visited_at) FROM threads WHERE group_id = g.id),
                         g.created_at)
         FROM groups g JOIN projects p ON p.id = g.project_id ORDER BY g.id",
        [],
        |row| Ok((row.get::<_, i64>(0)?, migrated_session(row, 1)?)),
    )?;
    for (group, session) in groups {
        let threads = collect(
            conn,
            "SELECT id, cwd, harness, short_id, session_id FROM threads
             WHERE group_id = ?1 ORDER BY created_at DESC, id DESC",
            [group],
            migrated_pane,
        )?;
        let panes: Vec<NewPane> = if threads.is_empty() {
            vec![NewPane::shell(session.dir.clone())]
        } else {
            threads
        };
        let (id, _) = insert_session(conn, &session, &panes)?;
        group_sessions.insert(group, id);
    }
    let mut thread_sessions = HashMap::new();
    let lone = collect(
        conn,
        "SELECT t.id, t.project_id,
                CASE p.kind WHEN 'incognito' THEN 'incognito' ELSE 'plain' END,
                t.cwd, t.renamed_title, t.branch, t.created_at, t.pinned_at,
                t.settled_override, t.settled_at, t.unsettled_at, t.last_activity_at,
                t.last_visited_at, t.id, t.cwd, t.harness, t.short_id, t.session_id
         FROM threads t JOIN projects p ON p.id = t.project_id
         WHERE t.group_id IS NULL OR t.group_id NOT IN (SELECT id FROM groups)
         ORDER BY t.id",
        [],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                migrated_session(row, 1)?,
                migrated_pane_at(row, 13)?,
            ))
        },
    )?;
    for (thread, session, pane) in lone {
        let (id, _) = insert_session(conn, &session, &[pane])?;
        thread_sessions.insert(thread, id);
    }
    let grouped = collect(
        conn,
        "SELECT id, group_id FROM threads WHERE group_id IS NOT NULL",
        [],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
    )?;
    for (thread, group) in grouped {
        if let Some(session) = group_sessions.get(&group) {
            thread_sessions.insert(thread, *session);
        }
    }
    rewrite_jumps(conn, &thread_sessions, &group_sessions)
}

/// Points thread and group jump rows at their sessions and deletes the rest
/// (drafts, rows whose thread or group is gone).
fn rewrite_jumps(
    conn: &Connection,
    thread_sessions: &HashMap<i64, SessionId>,
    group_sessions: &HashMap<i64, SessionId>,
) -> rusqlite::Result<()> {
    let jumps = collect(
        conn,
        "SELECT position, kind, item_id FROM jumps ORDER BY position",
        [],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        },
    )?;
    for (position, kind, item) in jumps {
        let session = match kind.as_str() {
            "thread" => thread_sessions.get(&item),
            "group" | "group_draft" => group_sessions.get(&item),
            _ => None,
        };
        match session {
            Some(session) => conn.execute(
                "UPDATE jumps SET kind = 'session', item_id = ?2 WHERE position = ?1",
                params![position, session.0],
            )?,
            None => conn.execute("DELETE FROM jumps WHERE position = ?1", [position])?,
        };
    }
    Ok(())
}

/// Every row `sql` returns, mapped.
fn collect<T, P, F>(conn: &Connection, sql: &str, params: P, map: F) -> rusqlite::Result<Vec<T>>
where
    P: rusqlite::Params,
    F: FnMut(&Row<'_>) -> rusqlite::Result<T>,
{
    conn.prepare(sql)?.query_map(params, map)?.collect()
}

/// A session to create: the `sessions` row's columns.
struct NewSession {
    project_id: i64,
    /// `plain`, `research`, `learn` or `incognito`.
    kind: String,
    dir: String,
    name: Option<String>,
    branch: Option<String>,
    created_at: i64,
    pinned_at: Option<i64>,
    settled_override: Option<String>,
    settled_at: Option<i64>,
    unsettled_at: Option<i64>,
    last_activity_at: i64,
    last_visited_at: i64,
}

/// A pane to create in a new session, and the thread that runs in it.
struct NewPane {
    thread: Option<i64>,
    cwd: String,
    zmx_name: Option<String>,
    zmx_dir: Option<String>,
    resume: Option<String>,
    migrated_bg: Option<String>,
}

impl NewPane {
    /// A pane running a shell in `cwd`, with no thread.
    fn shell(cwd: String) -> Self {
        Self {
            thread: None,
            cwd,
            zmx_name: None,
            zmx_dir: None,
            resume: None,
            migrated_bg: None,
        }
    }
}

/// A migrated session from the 12 columns starting at `at`: project, kind,
/// dir, name, branch, created, pinned, settle override, settled, un-settled,
/// last activity and last visit. A group kind other than Research or Learn
/// becomes `plain`.
fn migrated_session(row: &Row<'_>, at: usize) -> rusqlite::Result<NewSession> {
    let kind = match row.get::<_, String>(at + 1)?.as_str() {
        kind @ ("research" | "learn" | "incognito") => kind.to_owned(),
        _ => "plain".to_owned(),
    };
    Ok(NewSession {
        project_id: row.get(at)?,
        kind,
        dir: row.get(at + 2)?,
        name: row.get(at + 3)?,
        branch: row.get(at + 4)?,
        created_at: row.get(at + 5)?,
        pinned_at: row.get(at + 6)?,
        settled_override: row.get(at + 7)?,
        settled_at: row.get(at + 8)?,
        unsettled_at: row.get(at + 9)?,
        last_activity_at: row.get(at + 10)?,
        last_visited_at: row.get(at + 11)?,
    })
}

fn migrated_pane(row: &Row<'_>) -> rusqlite::Result<NewPane> {
    migrated_pane_at(row, 0)
}

/// A migrated thread's pane from the 5 columns starting at `at`: thread id,
/// cwd, harness, short id and session id. A Claude pane starts as a shell
/// that will resume the session, its `--bg` session still to stop; a pi pane
/// keeps the zmx session pi already runs in, under `~/.orb/pi`.
fn migrated_pane_at(row: &Row<'_>, at: usize) -> rusqlite::Result<NewPane> {
    let short_id: String = row.get(at + 3)?;
    let session_id: Option<String> = row.get(at + 4)?;
    let shell = NewPane {
        thread: Some(row.get(at)?),
        ..NewPane::shell(row.get(at + 1)?)
    };
    Ok(match row.get::<_, String>(at + 2)?.as_str() {
        "claude" => NewPane {
            resume: session_id.map(|id| format!("claude --resume {id}")),
            migrated_bg: Some(short_id),
            ..shell
        },
        "pi" => NewPane {
            resume: Some(format!("pi --session-id {short_id}")),
            zmx_name: Some(short_id),
            zmx_dir: Some("pi".to_owned()),
            ..shell
        },
        _ => shell,
    })
}

/// Inserts `session` with a tab for each of `panes`, in order, the first tab
/// active, and points each pane's thread at its pane.
fn insert_session(
    conn: &Connection,
    session: &NewSession,
    panes: &[NewPane],
) -> rusqlite::Result<(SessionId, Vec<PaneId>)> {
    let id = SessionId(conn.query_row(
        "INSERT INTO sessions
           (project_id, kind, dir, name, branch, created_at, pinned_at, settled_override,
            settled_at, unsettled_at, last_activity_at, last_visited_at, active_tab)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 0) RETURNING id",
        params![
            session.project_id,
            session.kind,
            session.dir,
            session.name,
            session.branch,
            session.created_at,
            session.pinned_at,
            session.settled_override,
            session.settled_at,
            session.unsettled_at,
            session.last_activity_at,
            session.last_visited_at,
        ],
        |row| row.get(0),
    )?);
    let mut ids = Vec::with_capacity(panes.len());
    for pane in panes {
        let pane_id = PaneId(conn.query_row(
            "INSERT INTO panes (session_id, cwd, zmx_name, zmx_dir, resume, migrated_bg)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6) RETURNING id",
            params![
                id.0,
                pane.cwd,
                pane.zmx_name,
                pane.zmx_dir,
                pane.resume,
                pane.migrated_bg
            ],
            |row| row.get(0),
        )?);
        if let Some(thread) = pane.thread {
            conn.execute(
                "UPDATE threads SET pane_id = ?2, orb_session_id = ?3 WHERE id = ?1",
                params![thread, pane_id.0, id.0],
            )?;
        }
        conn.execute(
            "INSERT INTO tabs (session_id, position, layout, focus_pane) VALUES (?1, ?2, ?3, ?4)",
            params![
                id.0,
                ids.len(),
                TileLayout::from_saved(Node::Pane(pane_id), pane_id).to_json(),
                pane_id.0
            ],
        )?;
        ids.push(pane_id);
    }
    Ok((id, ids))
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
        removed_at: row.get(4)?,
        kind: project_kind(row.get::<_, Option<String>>(5)?.as_deref()),
    })
}

/// How a project's kind is saved; Normal is `NULL`.
fn project_kind_text(kind: ProjectKind) -> Option<&'static str> {
    match kind {
        ProjectKind::Normal => None,
        ProjectKind::Research => Some("research"),
        ProjectKind::Learn => Some("learn"),
        ProjectKind::Incognito => Some("incognito"),
    }
}

/// A saved project kind; `NULL` or unknown text loads as Normal.
fn project_kind(text: Option<&str>) -> ProjectKind {
    match text {
        Some("research") => ProjectKind::Research,
        Some("learn") => ProjectKind::Learn,
        Some("incognito") => ProjectKind::Incognito,
        _ => ProjectKind::Normal,
    }
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
        last_activity_at: row.get(12)?,
        last_visited_at: row.get(13)?,
        ai_titled: row.get(14)?,
        model: row.get(15)?,
        renamed_title: row.get(16)?,
        harness: HarnessId::new(row.get::<_, String>(17)?),
        pane_id: row.get::<_, Option<i64>>(18)?.map(PaneId),
        orb_session: row.get::<_, Option<i64>>(19)?.map(SessionId),
    })
}

/// The layout settings; a width beyond `u16` loads as the nearest bound.
fn ui_row(row: &Row<'_>) -> rusqlite::Result<Ui> {
    Ok(Ui {
        sidebar_width: row
            .get::<_, Option<i64>>(0)?
            .map(|width| u16::try_from(width.max(0)).unwrap_or(u16::MAX)),
        project_filter: row.get::<_, Option<i64>>(1)?.map(ProjectId),
    })
}

/// How session kind `kind` is saved.
fn session_kind_text(kind: SessionKind) -> &'static str {
    match kind {
        SessionKind::Plain => "plain",
        SessionKind::Research => "research",
        SessionKind::Learn => "learn",
        SessionKind::Incognito => "incognito",
    }
}

/// The session kind saved as `text`; anything orb doesn't know is Plain.
fn session_kind(text: &str) -> SessionKind {
    match text {
        "research" => SessionKind::Research,
        "learn" => SessionKind::Learn,
        "incognito" => SessionKind::Incognito,
        _ => SessionKind::Plain,
    }
}

/// How a tab's swap layout is saved.
fn swap_layout_text(layout: SwapLayout) -> &'static str {
    match layout {
        SwapLayout::Base => "base",
        SwapLayout::Vertical => "vertical",
        SwapLayout::Horizontal => "horizontal",
        SwapLayout::Stacked => "stacked",
        SwapLayout::HalfStacked => "half-stacked",
    }
}

/// The swap layout saved as `text`; anything orb doesn't know is vertical,
/// as tabs saved before swap layouts load.
fn swap_layout(text: &str) -> SwapLayout {
    match text {
        "base" => SwapLayout::Base,
        "horizontal" => SwapLayout::Horizontal,
        "stacked" => SwapLayout::Stacked,
        "half-stacked" => SwapLayout::HalfStacked,
        _ => SwapLayout::Vertical,
    }
}

/// How a jump-list row is saved: its kind and id; the Settled header and
/// agent rows aren't.
fn jump_kind(item: SidebarItem) -> Option<(&'static str, i64)> {
    match item {
        SidebarItem::Session(id) => Some(("session", id.0)),
        SidebarItem::SettledShelf | SidebarItem::Agent { .. } => None,
    }
}

/// The jump-list row saved as `kind` and `id`; `None` for an unknown kind.
fn jump_item(kind: &str, id: i64) -> Option<SidebarItem> {
    match kind {
        "session" => Some(SidebarItem::Session(SessionId(id))),
        _ => None,
    }
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "tests propagate store failures with `?` and assert on the outcome"
)]
mod tests {
    use crate::feat::harness::HarnessId;
    use std::path::{Path, PathBuf};

    use error_stack::{Report, ResultExt};
    use rusqlite::Connection;

    use crate::feat::layout::tree::SwapLayout;

    use super::{
        MIGRATIONS, NewPaneThread, PaneId, ProjectId, ProjectKind, SavedLayouts, SessionId,
        SessionKind, SessionRow, SettledOverride, SidebarItem, Store, StoreError, TabRow,
        ThreadRow, Ui, lock,
    };

    fn user_version(path: &Path) -> Result<usize, Report<StoreError>> {
        Connection::open(path)
            .and_then(|conn| conn.pragma_query_value(None, "user_version", |row| row.get(0)))
            .change_context(StoreError)
    }

    /// A thread and the one-pane session it runs in.
    struct Seeded {
        thread: super::ThreadId,
        session: SessionId,
        pane: PaneId,
    }

    /// Saves thread `28bf38e2` of Claude, created at 1000, in a new plain
    /// session of project `project_id` in `/tmp/orb`.
    fn insert_thread(store: &Store, project_id: ProjectId) -> Result<Seeded, Report<StoreError>> {
        let (session, pane) = store.insert_session(
            project_id,
            SessionKind::Plain,
            Path::new("/tmp/orb"),
            None,
            None,
            1_000,
        )?;
        let (thread, _) = store.insert_pane_thread(&NewPaneThread {
            pane,
            session_id: "28bf38e2".to_owned(),
            transcript_path: None,
            harness: HarnessId::new("claude"),
            cwd: PathBuf::from("/tmp/orb"),
            created_at: 1_000,
        })?;
        Ok(Seeded {
            thread,
            session,
            pane,
        })
    }

    #[rstest::rstest]
    fn lock_is_refused_while_another_holder_has_it() -> Result<(), Report<StoreError>> {
        // Given one orb holding the store's lock.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let path = dir.path().join("state.sqlite");
        let _held = lock(&path)?;

        // When a second orb tries to take it.
        let second = lock(&path);

        // Then it is refused.
        assert!(
            second.is_err(),
            "a second lock on the same store should fail"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn lock_is_free_again_once_its_holder_drops_it() -> Result<(), Report<StoreError>> {
        // Given an orb that held the store's lock and quit.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let path = dir.path().join("state.sqlite");
        drop(lock(&path)?);

        // When another orb takes it.
        let next = lock(&path);

        // Then it gets the lock.
        assert!(next.is_ok(), "a released lock should be free to take");
        Ok(())
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
            v1.apply(&conn).change_context(StoreError)?;
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
                sql.apply(&conn).change_context(StoreError)?;
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
    fn migrating_a_v3_database_adds_ai_titled() -> Result<(), Report<StoreError>> {
        // Given a database at schema version 3 holding a thread.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let path = dir.path().join("state.sqlite");
        {
            let conn = Connection::open(&path).change_context(StoreError)?;
            for sql in MIGRATIONS
                .get(..3)
                .ok_or_else(|| Report::new(StoreError).attach("no v3 migrations"))?
            {
                sql.apply(&conn).change_context(StoreError)?;
            }
            conn.execute_batch(
                "INSERT INTO projects (id, root, title, created_at) VALUES (1, '/tmp/orb', 'orb', 500);
                 INSERT INTO threads (project_id, short_id, cwd, created_at)
                 VALUES (1, '28bf38e2', '/tmp/orb', 1000);
                 PRAGMA user_version = 3;",
            )
            .change_context(StoreError)?;
        }

        // When opening the store and loading.
        let (_, threads) = Store::open(&path)?.load()?;

        // Then it's at the latest version and the thread isn't AI-titled.
        let ai_titled: Vec<bool> = threads.iter().map(|row| row.ai_titled).collect();
        assert_eq!(
            (user_version(&path)?, ai_titled),
            (MIGRATIONS.len(), vec![false]),
            "migration v4 should add ai_titled, false for existing threads"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn migrating_a_v4_database_adds_drafts_and_last_used() -> Result<(), Report<StoreError>> {
        // Given a database at schema version 4 holding a thread.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let path = dir.path().join("state.sqlite");
        {
            let conn = Connection::open(&path).change_context(StoreError)?;
            for sql in MIGRATIONS
                .get(..4)
                .ok_or_else(|| Report::new(StoreError).attach("no v4 migrations"))?
            {
                sql.apply(&conn).change_context(StoreError)?;
            }
            conn.execute_batch(
                "INSERT INTO projects (id, root, title, created_at) VALUES (1, '/tmp/orb', 'orb', 500);
                 INSERT INTO threads (project_id, short_id, cwd, created_at)
                 VALUES (1, '28bf38e2', '/tmp/orb', 1000);
                 PRAGMA user_version = 4;",
            )
            .change_context(StoreError)?;
        }

        // When opening the store.
        let (_, threads) = Store::open(&path)?.load()?;

        // Then it's at the latest version, the thread survives, and the new
        // table and columns exist.
        let conn = Connection::open(&path).change_context(StoreError)?;
        let has_schema = [
            "SELECT project_id, workspace, workspace_path, branch, model, permission_mode, created_at FROM drafts",
            "SELECT last_workspace, last_model, last_permission_mode, last_used_at FROM projects",
            "SELECT model, permission_mode FROM threads",
        ]
        .iter()
        .all(|sql| conn.prepare(sql).is_ok());
        let short_ids: Vec<String> = threads.into_iter().map(|row| row.short_id).collect();
        assert_eq!(
            (user_version(&path)?, short_ids, has_schema),
            (MIGRATIONS.len(), vec!["28bf38e2".to_owned()], true),
            "migration v5 should keep threads and add drafts and last-used columns"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn migrating_a_v5_database_adds_ui_and_removed_at() -> Result<(), Report<StoreError>> {
        // Given a database at schema version 5 holding a project, a thread and
        // a draft.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let path = dir.path().join("state.sqlite");
        {
            let conn = Connection::open(&path).change_context(StoreError)?;
            for sql in MIGRATIONS
                .get(..5)
                .ok_or_else(|| Report::new(StoreError).attach("no v5 migrations"))?
            {
                sql.apply(&conn).change_context(StoreError)?;
            }
            conn.execute_batch(
                "INSERT INTO projects (id, root, title, created_at) VALUES (1, '/tmp/orb', 'orb', 500);
                 INSERT INTO threads (project_id, short_id, cwd, created_at)
                 VALUES (1, '28bf38e2', '/tmp/orb', 1000);
                 INSERT INTO drafts (project_id, workspace, created_at) VALUES (1, 'local', 2000);
                 PRAGMA user_version = 5;",
            )
            .change_context(StoreError)?;
        }

        // When opening the store.
        let (projects, threads) = Store::open(&path)?.load()?;

        // Then it's at the latest version, every row survives, and the ui
        // table and removed_at column exist.
        let conn = Connection::open(&path).change_context(StoreError)?;
        let has_schema = [
            "SELECT id, sidebar_width, project_filter FROM ui",
            "SELECT removed_at FROM projects",
        ]
        .iter()
        .all(|sql| conn.prepare(sql).is_ok());
        let drafts: i64 = conn
            .query_row("SELECT COUNT(*) FROM drafts", [], |row| row.get(0))
            .change_context(StoreError)?;
        assert_eq!(
            (
                user_version(&path)?,
                projects.len(),
                threads.len(),
                drafts,
                has_schema
            ),
            (MIGRATIONS.len(), 1, 1, 1, true),
            "migration v6 should keep every row and add ui and projects.removed_at"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn migrating_a_v7_database_adds_groups_and_keeps_its_threads() -> Result<(), Report<StoreError>>
    {
        // Given a database at schema version 7 holding a project and a thread.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let path = dir.path().join("state.sqlite");
        {
            let conn = Connection::open(&path).change_context(StoreError)?;
            for sql in MIGRATIONS
                .get(..7)
                .ok_or_else(|| Report::new(StoreError).attach("no v7 migrations"))?
            {
                sql.apply(&conn).change_context(StoreError)?;
            }
            conn.execute_batch(
                "INSERT INTO projects (id, root, title, created_at) VALUES (1, '/tmp/orb', 'orb', 500);
                 INSERT INTO threads (project_id, short_id, cwd, created_at)
                 VALUES (1, '28bf38e2', '/tmp/orb', 1000);
                 PRAGMA user_version = 7;",
            )
            .change_context(StoreError)?;
        }

        // When opening the store and loading.
        let threads = Store::open(&path)?.load()?.1;

        // Then it's at the latest version, the thread survives, and the groups
        // table and kind and group_id columns exist.
        let conn = Connection::open(&path).change_context(StoreError)?;
        let has_schema = [
            "SELECT kind FROM projects",
            "SELECT id, name FROM groups",
            "SELECT group_id FROM threads",
        ]
        .iter()
        .all(|sql| conn.prepare(sql).is_ok());
        assert_eq!(
            (user_version(&path)?, threads.len(), has_schema),
            (MIGRATIONS.len(), 1, true),
            "migration v8 should keep threads and add groups, projects.kind and threads.group_id"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn migrating_a_v8_database_adds_jumps_and_keeps_its_threads() -> Result<(), Report<StoreError>>
    {
        // Given a database at schema version 8 holding a project and a thread.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let path = dir.path().join("state.sqlite");
        {
            let conn = Connection::open(&path).change_context(StoreError)?;
            for sql in MIGRATIONS
                .get(..8)
                .ok_or_else(|| Report::new(StoreError).attach("no v8 migrations"))?
            {
                sql.apply(&conn).change_context(StoreError)?;
            }
            conn.execute_batch(
                "INSERT INTO projects (id, root, title, created_at) VALUES (1, '/tmp/orb', 'orb', 500);
                 INSERT INTO threads (project_id, short_id, cwd, created_at)
                 VALUES (1, '28bf38e2', '/tmp/orb', 1000);
                 PRAGMA user_version = 8;",
            )
            .change_context(StoreError)?;
        }

        // When opening the store and loading.
        let store = Store::open(&path)?;
        let threads = store.load()?.1;

        // Then it's at the latest version, the thread survives, and the jump
        // list reads back empty.
        assert_eq!(
            (user_version(&path)?, threads.len(), store.jumps()?),
            (MIGRATIONS.len(), 1, Vec::new()),
            "migration v9 should keep threads and add an empty jumps table"
        );
        Ok(())
    }

    /// A database at schema version 9 holding the orb project with a draft,
    /// the groups `with-thread` (holding thread `28bf38e2`) and `empty`.
    fn v9_database(path: &Path) -> Result<(), Report<StoreError>> {
        let conn = Connection::open(path).change_context(StoreError)?;
        for sql in MIGRATIONS
            .get(..9)
            .ok_or_else(|| Report::new(StoreError).attach("no v9 migrations"))?
        {
            sql.apply(&conn).change_context(StoreError)?;
        }
        conn.execute_batch(
            "INSERT INTO projects (id, root, title, created_at) VALUES (1, '/tmp/orb', 'orb', 500);
             INSERT INTO drafts (project_id, workspace, created_at) VALUES (1, 'local', 600);
             INSERT INTO groups (id, project_id, kind, name, branch, created_at)
             VALUES (1, 1, 'feature', 'with-thread', 'with-thread', 700),
                    (2, 1, 'feature', 'empty', 'empty', 800);
             INSERT INTO threads (project_id, short_id, cwd, created_at, group_id)
             VALUES (1, '28bf38e2', '/tmp/orb', 1000, 1);
             PRAGMA user_version = 9;",
        )
        .change_context(StoreError)
    }

    #[rstest::rstest]
    fn migrating_a_v9_database_backfills_claude_as_every_harness() -> Result<(), Report<StoreError>>
    {
        // Given a v9 database with a thread, a draft and groups.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let path = dir.path().join("state.sqlite");
        v9_database(&path)?;

        // When opening the store and loading.
        let (_, threads) = Store::open(&path)?.load()?;
        let saved = |sql: &str| -> Result<Vec<String>, Report<StoreError>> {
            let conn = Connection::open(&path).change_context(StoreError)?;
            let mut statement = conn.prepare(sql).change_context(StoreError)?;
            statement
                .query_map([], |row| row.get(0))
                .and_then(Iterator::collect)
                .change_context(StoreError)
        };

        // Then every thread, draft and group runs in claude.
        let harnesses: Vec<String> = threads
            .iter()
            .map(|row| row.harness.to_string())
            .chain(saved("SELECT harness FROM drafts")?)
            .chain(saved("SELECT harness FROM groups ORDER BY id")?)
            .collect();
        assert_eq!(
            harnesses,
            vec!["claude"; 4],
            "migration v10 should store claude as the harness of everything saved before it"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn migrating_a_v9_database_gives_only_threadless_groups_a_draft()
    -> Result<(), Report<StoreError>> {
        // Given a v9 database with a group holding a thread and an empty one.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let path = dir.path().join("state.sqlite");
        v9_database(&path)?;

        // When opening the store.
        drop(Store::open(&path)?);

        // Then only the empty group has a draft.
        let has_draft: Vec<(i64, bool)> = {
            let conn = Connection::open(&path).change_context(StoreError)?;
            let mut statement = conn
                .prepare("SELECT id, has_draft FROM groups ORDER BY id")
                .change_context(StoreError)?;
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .and_then(Iterator::collect)
                .change_context(StoreError)?
        };
        assert_eq!(
            has_draft,
            vec![(1, false), (2, true)],
            "migration v10 should give a draft only to a group without threads"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn migrating_a_v11_database_loads_its_tabs_vertical_and_untouched()
    -> Result<(), Report<StoreError>> {
        // Given a database at schema version 11 holding one tab.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let path = dir.path().join("state.sqlite");
        {
            let conn = Connection::open(&path).change_context(StoreError)?;
            for sql in MIGRATIONS
                .get(..11)
                .ok_or_else(|| Report::new(StoreError).attach("no v11 migrations"))?
            {
                sql.apply(&conn).change_context(StoreError)?;
            }
            conn.execute_batch(
                "INSERT INTO projects (id, root, title, created_at) VALUES (1, '/tmp/orb', 'orb', 500);
                 INSERT INTO sessions (id, project_id, dir, created_at) VALUES (1, 1, '/tmp/orb', 1000);
                 INSERT INTO panes (id, session_id, cwd) VALUES (7, 1, '/tmp/orb');
                 INSERT INTO tabs (session_id, position, layout, focus_pane)
                 VALUES (1, 0, '{\"pane\":7}', 7);
                 PRAGMA user_version = 11;",
            )
            .change_context(StoreError)?;
        }

        // When opening the store and loading its layouts.
        let tabs: Vec<(String, SwapLayout, bool)> = Store::open(&path)?
            .layouts()?
            .tabs
            .into_iter()
            .map(|tab| (tab.layout, tab.swap_layout, tab.hand_changed))
            .collect();

        // Then it's at the latest version and the tab is vertical, unmarked
        // and keeps its tree.
        assert_eq!(
            (user_version(&path)?, tabs),
            (
                MIGRATIONS.len(),
                vec![("{\"pane\":7}".to_owned(), SwapLayout::Vertical, false)]
            ),
            "migration v12 should load old tabs vertical and untouched"
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
        let (project_id, inserted) = {
            let store = Store::open(&path)?;
            let project_id =
                store.add_project(Path::new("/tmp/orb"), "orb", ProjectKind::Normal, 500)?;
            (project_id, insert_thread(&store, project_id)?)
        };

        // When reopening the store and loading.
        let (_, threads) = Store::open(&path)?.load()?;

        // Then the thread comes back with the fields it was saved with.
        let expected = ThreadRow {
            harness: HarnessId::new("claude"),
            id: inserted.thread,
            pane_id: Some(inserted.pane),
            orb_session: Some(inserted.session),
            project_id,
            short_id: "28bf38e2".to_owned(),
            session_id: Some("28bf38e2".to_owned()),
            title: None,
            custom_title: None,
            cwd: PathBuf::from("/tmp/orb"),
            transcript_path: None,
            transcript_offset: 0,
            created_at: 1_000,
            turn_started_at: None,
            branch: None,
            last_activity_at: 1_000,
            last_visited_at: 1_000,
            ai_titled: false,
            model: None,
            renamed_title: None,
        };
        assert_eq!(threads, vec![expected], "the saved thread should load back");
        Ok(())
    }

    #[rstest::rstest]
    fn adding_the_same_root_twice_returns_the_same_id() -> Result<(), Report<StoreError>> {
        // Given a store with a project rooted at /tmp/orb.
        let store = Store::open_in_memory()?;
        let first = store.add_project(Path::new("/tmp/orb"), "orb", ProjectKind::Normal, 500)?;

        // When adding the same root again.
        let second = store.add_project(Path::new("/tmp/orb"), "orb", ProjectKind::Normal, 900)?;

        // Then it gets the same id.
        assert_eq!(first, second, "one root should be one project");
        Ok(())
    }

    #[rstest::rstest]
    fn adding_an_existing_root_keeps_its_title() -> Result<(), Report<StoreError>> {
        // Given a store with a project titled "T3 orb" rooted at /tmp/orb.
        let store = Store::open_in_memory()?;
        store.add_project(Path::new("/tmp/orb"), "T3 orb", ProjectKind::Normal, 500)?;

        // When adding the same root under the title "orb".
        store.add_project(Path::new("/tmp/orb"), "orb", ProjectKind::Normal, 900)?;

        // Then the project keeps its first title.
        let titles: Vec<String> = store
            .load()?
            .0
            .into_iter()
            .map(|project| project.title)
            .collect();
        assert_eq!(
            titles,
            vec!["T3 orb".to_owned()],
            "adding an existing root shouldn't retitle it"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn adding_an_existing_root_as_research_sets_its_kind() -> Result<(), Report<StoreError>> {
        // Given a store with a normal project rooted at /tmp/research.
        let store = Store::open_in_memory()?;
        store.add_project(
            Path::new("/tmp/research"),
            "research",
            ProjectKind::Normal,
            500,
        )?;

        // When adding the same root as Research.
        store.add_project(
            Path::new("/tmp/research"),
            "Research",
            ProjectKind::Research,
            900,
        )?;

        // Then the project loads as Research.
        let kinds: Vec<ProjectKind> = store
            .load()?
            .0
            .into_iter()
            .map(|project| project.kind)
            .collect();
        assert_eq!(
            kinds,
            vec![ProjectKind::Research],
            "adding an existing root as Research should set its kind"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn incognito_project_kind_loads_back() -> Result<(), Report<StoreError>> {
        // Given a store with an Incognito project.
        let store = Store::open_in_memory()?;
        store.add_project(
            Path::new("/tmp/orb-incognito"),
            "Incognito",
            ProjectKind::Incognito,
            500,
        )?;

        // When loading.
        let kinds: Vec<ProjectKind> = store
            .load()?
            .0
            .into_iter()
            .map(|project| project.kind)
            .collect();

        // Then its kind is Incognito.
        assert_eq!(
            kinds,
            vec![ProjectKind::Incognito],
            "an Incognito project should load back as Incognito"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn adding_a_research_root_again_as_normal_keeps_its_kind() -> Result<(), Report<StoreError>> {
        // Given a store with a Research project rooted at /tmp/research.
        let store = Store::open_in_memory()?;
        store.add_project(
            Path::new("/tmp/research"),
            "Research",
            ProjectKind::Research,
            500,
        )?;

        // When adding the same root as Normal.
        store.add_project(
            Path::new("/tmp/research"),
            "research",
            ProjectKind::Normal,
            900,
        )?;

        // Then the project still loads as Research.
        let kinds: Vec<ProjectKind> = store
            .load()?
            .0
            .into_iter()
            .map(|project| project.kind)
            .collect();
        assert_eq!(
            kinds,
            vec![ProjectKind::Research],
            "re-adding a Research root as Normal should keep its kind"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn removed_project_loads_with_when_it_was_removed() -> Result<(), Report<StoreError>> {
        // Given a store with a project.
        let store = Store::open_in_memory()?;
        let project_id =
            store.add_project(Path::new("/tmp/orb"), "orb", ProjectKind::Normal, 500)?;

        // When removing it at 2 s.
        store.remove_project(project_id, 2_000)?;

        // Then it loads as removed at 2 s.
        let removed: Vec<Option<i64>> = store
            .load()?
            .0
            .into_iter()
            .map(|project| project.removed_at)
            .collect();
        assert_eq!(removed, vec![Some(2_000)], "the removal should be saved");
        Ok(())
    }

    #[rstest::rstest]
    fn removing_a_project_keeps_its_threads() -> Result<(), Report<StoreError>> {
        // Given a store with a project that has a thread.
        let store = Store::open_in_memory()?;
        let project_id =
            store.add_project(Path::new("/tmp/orb"), "orb", ProjectKind::Normal, 500)?;
        insert_thread(&store, project_id)?;

        // When removing the project.
        store.remove_project(project_id, 2_000)?;

        // Then the thread still loads.
        assert_eq!(
            store.load()?.1.len(),
            1,
            "a removed project's threads should stay"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn adding_a_removed_root_restores_it() -> Result<(), Report<StoreError>> {
        // Given a store whose project at /tmp/orb was removed.
        let store = Store::open_in_memory()?;
        let project_id =
            store.add_project(Path::new("/tmp/orb"), "orb", ProjectKind::Normal, 500)?;
        store.remove_project(project_id, 2_000)?;

        // When adding the same root again.
        store.add_project(Path::new("/tmp/orb"), "orb", ProjectKind::Normal, 3_000)?;

        // Then it's no longer removed.
        let removed: Vec<Option<i64>> = store
            .load()?
            .0
            .into_iter()
            .map(|project| project.removed_at)
            .collect();
        assert_eq!(removed, vec![None], "re-adding should restore the project");
        Ok(())
    }

    #[rstest::rstest]
    fn adding_a_removed_root_keeps_its_id() -> Result<(), Report<StoreError>> {
        // Given a store whose project at /tmp/orb was removed.
        let store = Store::open_in_memory()?;
        let first = store.add_project(Path::new("/tmp/orb"), "orb", ProjectKind::Normal, 500)?;
        store.remove_project(first, 2_000)?;

        // When adding the same root again.
        let second = store.add_project(Path::new("/tmp/orb"), "orb", ProjectKind::Normal, 3_000)?;

        // Then it's the same project.
        assert_eq!(first, second, "a restored project should keep its id");
        Ok(())
    }

    #[rstest::rstest]
    fn saved_thread_updates_load_back() -> Result<(), Report<StoreError>> {
        // Given a store with one thread.
        let store = Store::open_in_memory()?;
        let project_id =
            store.add_project(Path::new("/tmp/orb"), "orb", ProjectKind::Normal, 500)?;
        let inserted = insert_thread(&store, project_id)?;

        // When saving its session id, titles, transcript cursor, and turn start.
        let updated = ThreadRow {
            orb_session: None,
            harness: HarnessId::new("claude"),
            id: inserted.thread,
            pane_id: Some(inserted.pane),
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
            last_activity_at: 1_000,
            last_visited_at: 1_000,
            ai_titled: false,
            model: None,
            renamed_title: None,
        };
        store.save_thread(&updated)?;

        // Then loading returns the updated values.
        let (_, threads) = store.load()?;
        assert_eq!(threads, vec![updated], "the updates should load back");
        Ok(())
    }

    #[rstest::rstest]
    fn renamed_title_loads_back_after_saving() -> Result<(), Report<StoreError>> {
        // Given a store with one thread.
        let store = Store::open_in_memory()?;
        let project_id =
            store.add_project(Path::new("/tmp/orb"), "orb", ProjectKind::Normal, 500)?;
        insert_thread(&store, project_id)?;
        let inserted = store
            .load()?
            .1
            .pop()
            .ok_or_else(|| Report::new(StoreError).attach("the thread wasn't saved"))?;

        // When saving the name the user gave it with `r`.
        store.save_thread(&ThreadRow {
            renamed_title: Some("Sidebar search".to_owned()),
            ..inserted
        })?;

        // Then loading returns that name.
        let renamed: Vec<_> = store
            .load()?
            .1
            .into_iter()
            .map(|row| row.renamed_title)
            .collect();
        assert_eq!(
            renamed,
            vec![Some("Sidebar search".to_owned())],
            "the orb name should load back"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn deleted_thread_is_gone_after_reload() -> Result<(), Report<StoreError>> {
        // Given a store with one thread.
        let store = Store::open_in_memory()?;
        let project_id =
            store.add_project(Path::new("/tmp/orb"), "orb", ProjectKind::Normal, 500)?;
        let thread_id = insert_thread(&store, project_id)?.thread;

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
        let project_id =
            store.add_project(Path::new("/tmp/orb"), "orb", ProjectKind::Normal, 500)?;

        // When inserting a thread created at 1 s.
        insert_thread(&store, project_id)?;

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
    fn saved_ui_reads_back() -> Result<(), Report<StoreError>> {
        // Given a store with a project.
        let store = Store::open_in_memory()?;
        let project_id =
            store.add_project(Path::new("/tmp/orb"), "orb", ProjectKind::Normal, 500)?;

        // When saving a 40-column sidebar filtered to the project.
        let ui = Ui {
            sidebar_width: Some(40),
            project_filter: Some(project_id),
        };
        store.save_ui(&ui)?;

        // Then the layout settings are those.
        assert_eq!(store.ui()?, ui, "the saved layout should read back");
        Ok(())
    }

    #[rstest::rstest]
    fn saving_ui_again_replaces_it() -> Result<(), Report<StoreError>> {
        // Given a store whose sidebar was saved 40 columns wide.
        let store = Store::open_in_memory()?;
        store.save_ui(&Ui {
            sidebar_width: Some(40),
            project_filter: None,
        })?;

        // When saving it 28 columns wide.
        let ui = Ui {
            sidebar_width: Some(28),
            project_filter: None,
        };
        store.save_ui(&ui)?;

        // Then the second save is what reads back.
        assert_eq!(store.ui()?, ui, "the latest layout should replace the old");
        Ok(())
    }

    #[rstest::rstest]
    fn unsaved_ui_is_the_default() -> Result<(), Report<StoreError>> {
        // Given a fresh store.
        let store = Store::open_in_memory()?;

        // When reading the layout settings.
        let ui = store.ui()?;

        // Then nothing is set.
        assert_eq!(ui, Ui::default(), "a fresh store has no layout settings");
        Ok(())
    }

    #[rstest::rstest]
    fn insert_session_saves_one_tab_with_one_shell_pane() -> Result<(), Report<StoreError>> {
        // Given a store with one project.
        let store = Store::open_in_memory()?;
        let project = store.add_project(Path::new("/tmp/orb"), "orb", ProjectKind::Normal, 500)?;

        // When inserting a session in the project's root.
        let (session, pane) = store.insert_session(
            project,
            SessionKind::Plain,
            Path::new("/tmp/orb"),
            None,
            None,
            1_000,
        )?;

        // Then its one tab focuses its one pane, a shell in that directory.
        let saved = store.layouts()?;
        let tabs: Vec<(SessionId, Option<PaneId>)> = saved
            .tabs
            .iter()
            .map(|tab| (tab.session_id, tab.focus_pane))
            .collect();
        let panes: Vec<(PaneId, SessionId, PathBuf, Option<String>)> = saved
            .panes
            .iter()
            .map(|row| (row.id, row.session_id, row.cwd.clone(), row.resume.clone()))
            .collect();
        assert_eq!(
            (tabs, panes),
            (
                vec![(session, Some(pane))],
                vec![(pane, session, PathBuf::from("/tmp/orb"), None)]
            ),
            "a new session should hold one tab of one shell pane"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn insert_session_saves_its_kind_and_branch() -> Result<(), Report<StoreError>> {
        // Given a store with one project.
        let store = Store::open_in_memory()?;
        let project = store.add_project(Path::new("/tmp/orb"), "orb", ProjectKind::Normal, 500)?;

        // When inserting a Research session on `orb/0a1b2c3d`.
        let (session, _) = store.insert_session(
            project,
            SessionKind::Research,
            Path::new("/tmp/orb/wt"),
            Some("orb/0a1b2c3d"),
            None,
            1_000,
        )?;

        // Then its row loads back with that kind, branch and directory.
        let rows: Vec<(SessionId, SessionKind, Option<String>, PathBuf, i64)> = store
            .layouts()?
            .sessions
            .into_iter()
            .map(|row| (row.id, row.kind, row.branch, row.dir, row.last_activity_at))
            .collect();
        assert_eq!(
            rows,
            vec![(
                session,
                SessionKind::Research,
                Some("orb/0a1b2c3d".to_owned()),
                PathBuf::from("/tmp/orb/wt"),
                1_000
            )],
            "the session row should keep its kind and branch"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn move_session_sets_dir_branch_and_pane_cwds() -> Result<(), Report<StoreError>> {
        // Given a session in the checkout with a second pane.
        let store = Store::open_in_memory()?;
        let project = store.add_project(Path::new("/tmp/orb"), "orb", ProjectKind::Normal, 500)?;
        let (session, _) = store.insert_session(
            project,
            SessionKind::Plain,
            Path::new("/tmp/orb"),
            None,
            None,
            1_000,
        )?;
        store.insert_pane(session, Path::new("/tmp/orb"))?;

        // When moving it to a worktree on `orb/0a1b2c3d`.
        store.move_session(session, Path::new("/tmp/wt"), Some("orb/0a1b2c3d"))?;

        // Then its row and every pane point at the worktree.
        let layouts = store.layouts()?;
        let moved = (
            layouts
                .sessions
                .iter()
                .map(|row| (row.dir.clone(), row.branch.clone()))
                .collect::<Vec<_>>(),
            layouts
                .panes
                .iter()
                .map(|row| row.cwd.clone())
                .collect::<Vec<_>>(),
        );
        assert_eq!(
            moved,
            (
                vec![(PathBuf::from("/tmp/wt"), Some("orb/0a1b2c3d".to_owned()))],
                vec![PathBuf::from("/tmp/wt"), PathBuf::from("/tmp/wt")],
            ),
            "the session and its panes should move together"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn jumps_round_trip_session_rows() -> Result<(), Report<StoreError>> {
        // Given a fresh store.
        let store = Store::open_in_memory()?;

        // When saving a jump list of every kind of row.
        let jumps = vec![
            SidebarItem::Session(SessionId(3)),
            SidebarItem::Session(SessionId(4)),
        ];
        store.save_jumps(&jumps)?;

        // Then the same rows read back in the same order.
        assert_eq!(
            store.jumps()?,
            jumps,
            "the saved jump list should read back"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn saved_jumps_skip_agent_rows() -> Result<(), Report<StoreError>> {
        // Given a fresh store.
        let store = Store::open_in_memory()?;

        // When saving an agent row and a session row.
        store.save_jumps(&[
            SidebarItem::Agent {
                session: SessionId(1),
                pane: PaneId(1),
            },
            SidebarItem::Session(SessionId(2)),
        ])?;

        // Then only the session row reads back.
        assert_eq!(
            store.jumps()?,
            vec![SidebarItem::Session(SessionId(2))],
            "agent rows should not be saved in the jump list"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn saving_jumps_again_replaces_them() -> Result<(), Report<StoreError>> {
        // Given a store with sessions 1 and 2 saved as the jump list.
        let store = Store::open_in_memory()?;
        store.save_jumps(&[
            SidebarItem::Session(SessionId(1)),
            SidebarItem::Session(SessionId(2)),
        ])?;

        // When saving session 3 alone.
        let jumps = vec![SidebarItem::Session(SessionId(3))];
        store.save_jumps(&jumps)?;

        // Then only session 3 reads back.
        assert_eq!(
            store.jumps()?,
            jumps,
            "the latest jump list should replace the old"
        );
        Ok(())
    }

    /// The pi thread's short id in the v10 fixture.
    const PI_SHORT_ID: &str = "orb-0123456789abcdef0123456789abcdef";

    /// A v10 database: project 1 `/work/orb` with a draft, project 2 the
    /// Incognito folder; lone Claude thread 3 (`aa11`, pinned, renamed), lone
    /// pi thread 4, Incognito thread 5, Feature group 1 in `/wt/x` holding
    /// threads 6, 7 and 8 (created in that order), settled Research group 2
    /// holding thread 9, threadless Feature group 3 with no dir, settled lone
    /// thread 10; jumps to thread 3, group 1, the draft and thread 9.
    fn v10_database(path: &Path) -> Result<(), Report<StoreError>> {
        let conn = Connection::open(path).change_context(StoreError)?;
        for migration in MIGRATIONS
            .get(..10)
            .ok_or_else(|| Report::new(StoreError).attach("no v10 migrations"))?
        {
            migration.apply(&conn).change_context(StoreError)?;
        }
        conn.execute_batch(&format!(
            "INSERT INTO projects (id, root, title, created_at, kind)
             VALUES (1, '/work/orb', 'orb', 100, NULL),
                    (2, '/tmp/orb-incognito', 'Incognito', 100, 'incognito');
             INSERT INTO drafts (project_id, workspace, created_at) VALUES (1, 'local', 150);
             INSERT INTO groups (id, project_id, kind, name, dir, branch, created_at,
                                 settled_override, settled_at)
             VALUES (1, 1, 'feature', 'login', '/wt/x', 'login', 200, NULL, NULL),
                    (2, 1, 'research', 'papers', '/r/papers', NULL, 300, 'settled', 8000),
                    (3, 1, 'feature', 'later', NULL, NULL, 400, NULL, NULL);
             INSERT INTO threads (id, project_id, short_id, session_id, cwd, created_at,
                                  renamed_title, pinned_at, settled_override, settled_at,
                                  last_activity_at, last_visited_at, group_id, harness)
             VALUES (3, 1, 'aa11', 's-aa11', '/work/orb', 1000, 'Fix login', 5000,
                     NULL, NULL, 6000, 7000, NULL, 'claude'),
                    (4, 1, '{PI_SHORT_ID}', NULL, '/work/orb', 1100, NULL, NULL,
                     NULL, NULL, 1100, 1100, NULL, 'pi'),
                    (5, 2, 'cc33', 's-cc33', '/tmp/orb-incognito', 1200, NULL, NULL,
                     NULL, NULL, 1200, 1200, NULL, 'claude'),
                    (6, 1, 'dd44', 's-dd44', '/wt/x', 1300, NULL, NULL,
                     NULL, NULL, 1300, 1300, 1, 'claude'),
                    (7, 1, 'ee55', 's-ee55', '/wt/x', 1400, NULL, NULL,
                     NULL, NULL, 1400, 1400, 1, 'claude'),
                    (8, 1, 'ff66', 's-ff66', '/wt/x', 1500, NULL, NULL,
                     NULL, NULL, 1500, 1500, 1, 'claude'),
                    (9, 1, 'gg77', 's-gg77', '/r/papers', 1600, NULL, NULL,
                     NULL, NULL, 1600, 1600, 2, 'claude'),
                    (10, 1, 'hh88', 's-hh88', '/work/orb', 1700, NULL, NULL,
                     'settled', 9000, 1700, 1700, NULL, 'claude');
             INSERT INTO jumps (position, kind, item_id)
             VALUES (0, 'thread', 3), (1, 'group', 1), (2, 'draft', 1), (3, 'thread', 9);
             PRAGMA user_version = 10;"
        ))
        .change_context(StoreError)
    }

    /// A v10 fixture at `<dir>/state.sqlite`, opened (and so migrated).
    fn migrated_v10(dir: &Path) -> Result<PathBuf, Report<StoreError>> {
        let path = dir.join("state.sqlite");
        v10_database(&path)?;
        drop(Store::open(&path)?);
        Ok(path)
    }

    /// Every row `sql` returns from the database at `path`, each column
    /// debug-printed.
    fn rows(path: &Path, sql: &str) -> Result<Vec<Vec<String>>, Report<StoreError>> {
        let conn = Connection::open(path).change_context(StoreError)?;
        let mut statement = conn.prepare(sql).change_context(StoreError)?;
        let columns = statement.column_count();
        statement
            .query_map([], |row| {
                (0..columns)
                    .map(|at| {
                        row.get::<_, rusqlite::types::Value>(at)
                            .map(|value| format!("{value:?}"))
                    })
                    .collect()
            })
            .and_then(Iterator::collect)
            .change_context(StoreError)
    }

    /// The one value `sql` returns from the database at `path`.
    fn value<T>(path: &Path, sql: &str) -> Result<T, Report<StoreError>>
    where
        T: rusqlite::types::FromSql,
    {
        Connection::open(path)
            .and_then(|conn| conn.query_row(sql, [], |row| row.get(0)))
            .change_context(StoreError)
    }

    /// The session thread `id` runs in, after migrating.
    fn session_of(path: &Path, thread: i64) -> Result<i64, Report<StoreError>> {
        value(
            path,
            &format!(
                "SELECT p.session_id FROM threads t JOIN panes p ON p.id = t.pane_id
                 WHERE t.id = {thread}"
            ),
        )
    }

    /// The pane thread `id` runs in, after migrating.
    fn pane_of(path: &Path, thread: i64) -> Result<i64, Report<StoreError>> {
        value(
            path,
            &format!("SELECT pane_id FROM threads WHERE id = {thread}"),
        )
    }

    fn backup_path(path: &Path) -> PathBuf {
        let mut name = path.as_os_str().to_owned();
        name.push(".bak-v10");
        PathBuf::from(name)
    }

    const THREAD_COLUMNS: &str = "SELECT * FROM threads ORDER BY id";

    #[rstest::rstest]
    fn migrating_v10_writes_the_backup_before_changing_anything() -> Result<(), Report<StoreError>>
    {
        // Given a v10 database and its threads as they were.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let path = dir.path().join("state.sqlite");
        v10_database(&path)?;
        let before = rows(&path, THREAD_COLUMNS)?;

        // When opening the store.
        drop(Store::open(&path)?);

        // Then the backup is a v10 database holding the same threads.
        let backup = backup_path(&path);
        assert_eq!(
            (user_version(&backup)?, rows(&backup, THREAD_COLUMNS)?),
            (10, before),
            "the backup should be the v10 database as it was"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn migrating_v10_keeps_an_existing_backup() -> Result<(), Report<StoreError>> {
        // Given a v10 database with a backup already beside it.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let path = dir.path().join("state.sqlite");
        v10_database(&path)?;
        let backup = backup_path(&path);
        std::fs::write(&backup, b"an earlier backup").change_context(StoreError)?;

        // When opening the store.
        drop(Store::open(&path)?);

        // Then the backup is left as it was.
        assert_eq!(
            std::fs::read(&backup).change_context(StoreError)?,
            b"an earlier backup".to_vec(),
            "an existing backup should never be overwritten"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn failed_backup_leaves_a_v10_database_unmigrated() -> Result<(), Report<StoreError>> {
        // Given a v10 database whose backup can't be written (a directory
        // sits where the partial copy goes).
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let path = dir.path().join("state.sqlite");
        v10_database(&path)?;
        let mut partial = backup_path(&path).into_os_string();
        partial.push(".tmp");
        std::fs::create_dir_all(PathBuf::from(partial).join("blocker"))
            .change_context(StoreError)?;

        // When opening the store.
        let opened = Store::open(&path);

        // Then opening fails and the database is still at v10.
        assert_eq!(
            (opened.is_err(), user_version(&path)?),
            (true, 10),
            "no migration should run without a backup"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn opening_a_fresh_database_writes_no_backup() -> Result<(), Report<StoreError>> {
        // Given a path with no database.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let path = dir.path().join("state.sqlite");

        // When opening the store.
        drop(Store::open(&path)?);

        // Then the directory holds only the database.
        let names: Vec<String> = std::fs::read_dir(dir.path())
            .change_context(StoreError)?
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains(".bak"))
            .collect();
        assert!(
            names.is_empty(),
            "a fresh database needs no backup: {names:?}"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn migrating_v10_keeps_every_thread_id() -> Result<(), Report<StoreError>> {
        // Given a migrated v10 database.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let path = migrated_v10(dir.path())?;

        // When reading its threads.
        let threads = rows(
            &path,
            "SELECT id, pane_id IS NOT NULL FROM threads ORDER BY id",
        )?;

        // Then threads 3 to 10 are there, each in a pane.
        let expected: Vec<Vec<String>> = (3..=10)
            .map(|id| vec![format!("Integer({id})"), "Integer(1)".to_owned()])
            .collect();
        assert_eq!(
            threads, expected,
            "every thread should keep its id and get a pane"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn migrating_v10_records_each_threads_session() -> Result<(), Report<StoreError>> {
        // Given a migrated v10 database.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let path = migrated_v10(dir.path())?;

        // When reading which threads' session differs from their pane's.
        let mismatched = rows(
            &path,
            "SELECT t.id FROM threads t LEFT JOIN panes p ON p.id = t.pane_id
             WHERE t.orb_session_id IS NOT p.session_id OR t.orb_session_id IS NULL",
        )?;

        // Then every thread records the session of its pane.
        assert!(
            mismatched.is_empty(),
            "every migrated thread should record its pane's session: {mismatched:?}"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn migrating_v10_gives_a_lone_claude_thread_one_session_tab_and_pane()
    -> Result<(), Report<StoreError>> {
        // Given a migrated v10 database.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let path = migrated_v10(dir.path())?;

        // When reading thread 3's session and its tabs.
        let session = session_of(&path, 3)?;
        let pane = pane_of(&path, 3)?;
        let found = rows(
            &path,
            &format!(
                "SELECT s.dir, s.name, s.active_tab, t.position, t.layout, t.focus_pane,
                        (SELECT COUNT(*) FROM panes WHERE session_id = s.id)
                 FROM sessions s JOIN tabs t ON t.session_id = s.id WHERE s.id = {session}"
            ),
        )?;

        // Then it has one tab of its one pane, in the thread's directory.
        let expected = vec![vec![
            "Text(\"/work/orb\")".to_owned(),
            "Text(\"Fix login\")".to_owned(),
            "Integer(0)".to_owned(),
            "Integer(0)".to_owned(),
            format!("Text(\"{{\\\"pane\\\":{pane}}}\")"),
            format!("Integer({pane})"),
            "Integer(1)".to_owned(),
        ]];
        assert_eq!(
            found, expected,
            "a lone thread should become a one-pane session"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn migrating_v10_records_a_claude_panes_resume_and_bg_session() -> Result<(), Report<StoreError>>
    {
        // Given a migrated v10 database.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let path = migrated_v10(dir.path())?;

        // When reading thread 3's pane.
        let pane = pane_of(&path, 3)?;
        let found = rows(
            &path,
            &format!(
                "SELECT cwd, zmx_name, zmx_dir, resume, migrated_bg FROM panes WHERE id = {pane}"
            ),
        )?;

        // Then it resumes the Claude session and still has its --bg session to stop.
        let expected = vec![vec![
            "Text(\"/work/orb\")".to_owned(),
            "Null".to_owned(),
            "Null".to_owned(),
            "Text(\"claude --resume s-aa11\")".to_owned(),
            "Text(\"aa11\")".to_owned(),
        ]];
        assert_eq!(found, expected, "a Claude pane should resume its session");
        Ok(())
    }

    #[rstest::rstest]
    fn migrating_v10_keeps_a_pi_panes_zmx_session() -> Result<(), Report<StoreError>> {
        // Given a migrated v10 database.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let path = migrated_v10(dir.path())?;

        // When reading pi thread 4's pane.
        let pane = pane_of(&path, 4)?;
        let found = rows(
            &path,
            &format!("SELECT zmx_name, zmx_dir, resume, migrated_bg FROM panes WHERE id = {pane}"),
        )?;

        // Then it keeps pi's zmx session under orb's pi dir.
        let expected = vec![vec![
            format!("Text(\"{PI_SHORT_ID}\")"),
            "Text(\"pi\")".to_owned(),
            format!("Text(\"pi --session-id {PI_SHORT_ID}\")"),
            "Null".to_owned(),
        ]];
        assert_eq!(found, expected, "a pi pane should keep its zmx session");
        Ok(())
    }

    #[rstest::rstest]
    fn migrating_v10_puts_a_groups_threads_in_one_session() -> Result<(), Report<StoreError>> {
        // Given a migrated v10 database.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let path = migrated_v10(dir.path())?;

        // When reading the sessions of group 1's threads.
        let sessions: Vec<i64> = [6, 7, 8]
            .into_iter()
            .map(|thread| session_of(&path, thread))
            .collect::<Result<_, _>>()?;

        // Then all three are in the same session.
        assert!(
            sessions.windows(2).all(|pair| pair.first() == pair.last()),
            "a group's threads should share one session, got {sessions:?}"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn migrating_v10_gives_each_of_a_groups_threads_a_tab_newest_first()
    -> Result<(), Report<StoreError>> {
        // Given a migrated v10 database.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let path = migrated_v10(dir.path())?;

        // When reading the tabs of group 1's session.
        let session = session_of(&path, 6)?;
        let found = rows(
            &path,
            &format!(
                "SELECT position, name, layout, focus_pane FROM tabs
                 WHERE session_id = {session} ORDER BY position"
            ),
        )?;

        // Then each thread's pane is alone in its own unnamed tab, newest thread first.
        let expected: Vec<Vec<String>> = [8, 7, 6]
            .into_iter()
            .enumerate()
            .map(|(position, thread)| {
                pane_of(&path, thread).map(|pane| {
                    vec![
                        format!("Integer({position})"),
                        "Null".to_owned(),
                        format!("Text(\"{{\\\"pane\\\":{pane}}}\")"),
                        format!("Integer({pane})"),
                    ]
                })
            })
            .collect::<Result<_, _>>()?;
        assert_eq!(
            found, expected,
            "a group should become one tab per thread, newest first"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn migrating_v10_opens_a_groups_session_on_its_first_tab() -> Result<(), Report<StoreError>> {
        // Given a migrated v10 database.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let path = migrated_v10(dir.path())?;

        // When reading group 1's session's active tab.
        let session = session_of(&path, 6)?;
        let active: i64 = value(
            &path,
            &format!("SELECT active_tab FROM sessions WHERE id = {session}"),
        )?;

        // Then it is the first tab, the newest thread's.
        assert_eq!(active, 0, "a group's session should open on its first tab");
        Ok(())
    }

    #[rstest::rstest]
    fn migrating_v10_copies_a_settled_groups_settle_state() -> Result<(), Report<StoreError>> {
        // Given a migrated v10 database.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let path = migrated_v10(dir.path())?;

        // When reading the session of settled group 2's thread.
        let session = session_of(&path, 9)?;
        let found = rows(
            &path,
            &format!("SELECT settled_override, settled_at FROM sessions WHERE id = {session}"),
        )?;

        // Then it is settled when the group was.
        let expected = vec![vec![
            "Text(\"settled\")".to_owned(),
            "Integer(8000)".to_owned(),
        ]];
        assert_eq!(
            found, expected,
            "a settled group's session should stay settled"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn migrating_v10_copies_a_settled_threads_settle_state() -> Result<(), Report<StoreError>> {
        // Given a migrated v10 database.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let path = migrated_v10(dir.path())?;

        // When reading settled thread 10's session.
        let session = session_of(&path, 10)?;
        let found = rows(
            &path,
            &format!("SELECT settled_override, settled_at FROM sessions WHERE id = {session}"),
        )?;

        // Then it is settled when the thread was.
        let expected = vec![vec![
            "Text(\"settled\")".to_owned(),
            "Integer(9000)".to_owned(),
        ]];
        assert_eq!(
            found, expected,
            "a settled thread's session should stay settled"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn migrating_v10_copies_a_pinned_threads_pin_activity_and_visit()
    -> Result<(), Report<StoreError>> {
        // Given a migrated v10 database.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let path = migrated_v10(dir.path())?;

        // When reading pinned thread 3's session.
        let session = session_of(&path, 3)?;
        let found = rows(
            &path,
            &format!(
                "SELECT pinned_at, last_activity_at, last_visited_at FROM sessions
                 WHERE id = {session}"
            ),
        )?;

        // Then it has the thread's pin, activity and visit times.
        let expected = vec![vec![
            "Integer(5000)".to_owned(),
            "Integer(6000)".to_owned(),
            "Integer(7000)".to_owned(),
        ]];
        assert_eq!(
            found, expected,
            "a thread's pin, activity and visit should carry over"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn migrating_v10_makes_an_incognito_threads_session_incognito() -> Result<(), Report<StoreError>>
    {
        // Given a migrated v10 database.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let path = migrated_v10(dir.path())?;

        // When reading Incognito thread 5's session.
        let session = session_of(&path, 5)?;
        let kind: String = value(
            &path,
            &format!("SELECT kind FROM sessions WHERE id = {session}"),
        )?;

        // Then it is an Incognito session.
        assert_eq!(
            kind, "incognito",
            "an Incognito thread's session should be Incognito"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[case::feature(6, "plain")]
    #[case::research(9, "research")]
    fn migrating_v10_maps_group_kinds(
        #[case] thread: i64,
        #[case] expected: &str,
    ) -> Result<(), Report<StoreError>> {
        // Given a migrated v10 database.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let path = migrated_v10(dir.path())?;

        // When reading the kind of the session the group's thread is in.
        let session = session_of(&path, thread)?;
        let kind: String = value(
            &path,
            &format!("SELECT kind FROM sessions WHERE id = {session}"),
        )?;

        // Then it is the group's kind, a Feature becoming plain.
        assert_eq!(
            kind, expected,
            "a group's kind should map to a session kind"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn migrating_v10_gives_a_threadless_group_one_pane_in_the_project_root()
    -> Result<(), Report<StoreError>> {
        // Given a migrated v10 database.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let path = migrated_v10(dir.path())?;

        // When reading threadless group 3's session and panes.
        let found = rows(
            &path,
            "SELECT s.dir, p.cwd, p.resume FROM sessions s JOIN panes p ON p.session_id = s.id
             WHERE s.name = 'later'",
        )?;

        // Then it has one shell pane in the project's root.
        let expected = vec![vec![
            "Text(\"/work/orb\")".to_owned(),
            "Text(\"/work/orb\")".to_owned(),
            "Null".to_owned(),
        ]];
        assert_eq!(
            found, expected,
            "a threadless group should get one shell pane"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn migrating_v10_rewrites_jumps_to_sessions() -> Result<(), Report<StoreError>> {
        // Given a migrated v10 database.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let path = migrated_v10(dir.path())?;

        // When reading its jump rows.
        let jumps = rows(&path, "SELECT kind, item_id FROM jumps ORDER BY position")?;

        // Then thread and group rows point at their sessions and the draft's row is gone.
        let expected: Vec<Vec<String>> = [
            session_of(&path, 3)?,
            session_of(&path, 6)?,
            session_of(&path, 9)?,
        ]
        .into_iter()
        .map(|session| {
            vec![
                "Text(\"session\")".to_owned(),
                format!("Integer({session})"),
            ]
        })
        .collect();
        assert_eq!(jumps, expected, "jumps should point at sessions");
        Ok(())
    }

    #[rstest::rstest]
    fn migrating_v10_numbers_panes_above_every_thread_id() -> Result<(), Report<StoreError>> {
        // Given a migrated v10 database whose highest thread id is 10.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let path = migrated_v10(dir.path())?;

        // When reading the lowest pane id.
        let lowest: i64 = value(&path, "SELECT MIN(id) FROM panes")?;

        // Then it is above every thread id.
        assert!(
            lowest > 10,
            "pane ids should start above thread ids, got {lowest}"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn migrating_v10_gives_a_thread_of_a_missing_group_its_own_session()
    -> Result<(), Report<StoreError>> {
        // Given a v10 database with a thread whose group row is gone.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let path = dir.path().join("state.sqlite");
        v10_database(&path)?;
        Connection::open(&path)
            .and_then(|conn| {
                conn.execute_batch(
                    "INSERT INTO threads (id, project_id, short_id, cwd, created_at, group_id)
                     VALUES (11, 1, 'ii99', '/work/orb', 1800, 42)",
                )
            })
            .change_context(StoreError)?;

        // When opening the store.
        drop(Store::open(&path)?);

        // Then that thread runs in a one-pane session of its own.
        let panes: i64 = value(
            &path,
            &format!(
                "SELECT COUNT(*) FROM panes WHERE session_id = {}",
                session_of(&path, 11)?
            ),
        )?;
        assert_eq!(panes, 1, "a thread of a missing group should get a session");
        Ok(())
    }

    #[rstest::rstest]
    fn reopening_a_migrated_v10_database_writes_no_second_backup() -> Result<(), Report<StoreError>>
    {
        // Given a migrated v10 database whose backup was then removed.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let path = migrated_v10(dir.path())?;
        std::fs::remove_file(backup_path(&path)).change_context(StoreError)?;

        // When opening the store again.
        drop(Store::open(&path)?);

        // Then no backup is written.
        assert!(
            !backup_path(&path).exists(),
            "a database already at v11 should not be backed up again"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn reopening_a_migrated_v10_database_makes_no_more_sessions() -> Result<(), Report<StoreError>>
    {
        // Given a migrated v10 database and its session count.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let path = migrated_v10(dir.path())?;
        let before: i64 = value(&path, "SELECT COUNT(*) FROM sessions")?;

        // When opening the store again.
        drop(Store::open(&path)?);

        // Then the sessions are the same.
        let after: i64 = value(&path, "SELECT COUNT(*) FROM sessions")?;
        assert_eq!(after, before, "the migration should run only once");
        Ok(())
    }

    #[rstest::rstest]
    fn migrating_v10_keeps_groups_and_drafts() -> Result<(), Report<StoreError>> {
        // Given a migrated v10 database.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let path = migrated_v10(dir.path())?;

        // When counting its groups and drafts.
        let counts = (
            value::<i64>(&path, "SELECT COUNT(*) FROM groups")?,
            value::<i64>(&path, "SELECT COUNT(*) FROM drafts")?,
        );

        // Then every group and the draft are still there.
        assert_eq!(
            counts,
            (3, 1),
            "the migration should keep groups and drafts"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn inserted_thread_gets_a_session_with_one_pane() -> Result<(), Report<StoreError>> {
        // Given a store with a project.
        let store = Store::open_in_memory()?;
        let project_id =
            store.add_project(Path::new("/tmp/orb"), "orb", ProjectKind::Normal, 500)?;

        // When inserting a thread.
        let inserted = insert_thread(&store, project_id)?;

        // Then its session has one tab holding its one pane, in its directory.
        let layouts = store.layouts()?;
        let tabs: Vec<(SessionId, String, Option<PaneId>)> = layouts
            .tabs
            .into_iter()
            .map(|tab| (tab.session_id, tab.layout, tab.focus_pane))
            .collect();
        let panes: Vec<(PaneId, SessionId, PathBuf)> = layouts
            .panes
            .into_iter()
            .map(|pane| (pane.id, pane.session_id, pane.cwd))
            .collect();
        assert_eq!(
            (tabs, panes),
            (
                vec![(
                    inserted.session,
                    format!("{{\"pane\":{}}}", inserted.pane.0),
                    Some(inserted.pane)
                )],
                vec![(inserted.pane, inserted.session, PathBuf::from("/tmp/orb"))]
            ),
            "a new thread should get a one-pane session"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn saved_layout_loads_back() -> Result<(), Report<StoreError>> {
        // Given a thread's session with a second pane.
        let store = Store::open_in_memory()?;
        let project_id =
            store.add_project(Path::new("/tmp/orb"), "orb", ProjectKind::Normal, 500)?;
        let inserted = insert_thread(&store, project_id)?;
        let second = store.insert_pane(inserted.session, Path::new("/tmp/orb"))?;

        // When saving two tabs, the second active, and a name for the second pane.
        let tabs = vec![
            TabRow {
                session_id: inserted.session,
                position: 0,
                name: Some("agent".to_owned()),
                layout: format!("{{\"pane\":{}}}", inserted.pane.0),
                focus_pane: Some(inserted.pane),
                swap_layout: SwapLayout::Vertical,
                hand_changed: false,
            },
            TabRow {
                session_id: inserted.session,
                position: 1,
                name: None,
                layout: format!("{{\"pane\":{}}}", second.0),
                focus_pane: Some(second),
                swap_layout: SwapLayout::Vertical,
                hand_changed: false,
            },
        ];
        store.save_layout(
            inserted.session,
            1,
            &tabs,
            &[(inserted.pane, None), (second, Some("logs".to_owned()))],
        )?;

        // Then the tabs, the active tab and the name load back.
        let layouts = store.layouts()?;
        let active: Vec<usize> = layouts.sessions.iter().map(|row| row.active_tab).collect();
        let names: Vec<Option<String>> = layouts.panes.into_iter().map(|pane| pane.name).collect();
        assert_eq!(
            (layouts.tabs, active, names),
            (tabs, vec![1], vec![None, Some("logs".to_owned())]),
            "the saved layout should load back"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn saved_tab_keeps_its_swap_layout_and_mark() -> Result<(), Report<StoreError>> {
        // Given a thread's session.
        let store = Store::open_in_memory()?;
        let project_id =
            store.add_project(Path::new("/tmp/orb"), "orb", ProjectKind::Normal, 500)?;
        let inserted = insert_thread(&store, project_id)?;

        // When saving its tab half-stacked and changed by hand.
        let tab = TabRow {
            session_id: inserted.session,
            position: 0,
            name: None,
            layout: format!("{{\"pane\":{}}}", inserted.pane.0),
            focus_pane: Some(inserted.pane),
            swap_layout: SwapLayout::HalfStacked,
            hand_changed: true,
        };
        store.save_layout(inserted.session, 0, &[tab], &[(inserted.pane, None)])?;

        // Then the tab loads back half-stacked and marked.
        let tabs: Vec<(SwapLayout, bool)> = store
            .layouts()?
            .tabs
            .into_iter()
            .map(|tab| (tab.swap_layout, tab.hand_changed))
            .collect();
        assert_eq!(
            tabs,
            vec![(SwapLayout::HalfStacked, true)],
            "the tab's swap layout and mark should load back"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn saved_session_lifecycle_is_loaded_back() -> Result<(), Report<StoreError>> {
        // Given a thread's session.
        let store = Store::open_in_memory()?;
        let project_id =
            store.add_project(Path::new("/tmp/orb"), "orb", ProjectKind::Normal, 500)?;
        let inserted = insert_thread(&store, project_id)?;
        let before = store
            .layouts()?
            .sessions
            .into_iter()
            .find(|row| row.id == inserted.session)
            .ok_or_else(|| Report::new(StoreError).attach("the session isn't saved"))?;

        // When saving it named, pinned, settled and with new activity.
        let changed = SessionRow {
            name: Some("auth".to_owned()),
            pinned_at: Some(1_000),
            settled_override: Some(SettledOverride::Settled),
            settled_at: Some(2_000),
            unsettled_at: Some(1_500),
            last_activity_at: 3_000,
            ..before
        };
        store.save_session(&changed)?;

        // Then it loads back as saved.
        assert_eq!(
            store.layouts()?.sessions,
            vec![changed],
            "the session's lifecycle should load back"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn saving_a_layout_without_a_pane_deletes_its_row() -> Result<(), Report<StoreError>> {
        // Given a thread's session with a second pane.
        let store = Store::open_in_memory()?;
        let project_id =
            store.add_project(Path::new("/tmp/orb"), "orb", ProjectKind::Normal, 500)?;
        let inserted = insert_thread(&store, project_id)?;
        let second = store.insert_pane(inserted.session, Path::new("/tmp/orb"))?;

        // When saving a layout holding only the thread's pane.
        let tab = TabRow {
            session_id: inserted.session,
            position: 0,
            name: None,
            layout: format!("{{\"pane\":{}}}", inserted.pane.0),
            focus_pane: Some(inserted.pane),
            swap_layout: SwapLayout::Vertical,
            hand_changed: false,
        };
        let dropped = store.save_layout(inserted.session, 0, &[tab], &[(inserted.pane, None)])?;

        // Then the second pane's row is gone.
        let panes: Vec<PaneId> = store
            .layouts()?
            .panes
            .into_iter()
            .map(|pane| pane.id)
            .collect();
        assert_eq!(
            (dropped, panes),
            (vec![second], vec![inserted.pane]),
            "a pane no tab holds should be deleted"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn deleted_session_takes_its_tabs_and_panes() -> Result<(), Report<StoreError>> {
        // Given a thread's session.
        let store = Store::open_in_memory()?;
        let project_id =
            store.add_project(Path::new("/tmp/orb"), "orb", ProjectKind::Normal, 500)?;
        let inserted = insert_thread(&store, project_id)?;

        // When deleting the session.
        store.delete_session(inserted.session)?;

        // Then no session, tab or pane is left.
        assert_eq!(
            store.layouts()?,
            SavedLayouts::default(),
            "a deleted session should take its tabs and panes"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn deleted_session_takes_its_threads() -> Result<(), Report<StoreError>> {
        // Given a thread's session.
        let store = Store::open_in_memory()?;
        let project_id =
            store.add_project(Path::new("/tmp/orb"), "orb", ProjectKind::Normal, 500)?;
        let inserted = insert_thread(&store, project_id)?;

        // When deleting the session.
        store.delete_session(inserted.session)?;

        // Then the thread that ran in it is gone too.
        let (_, threads, ..) = store.load()?;
        assert!(
            threads.is_empty(),
            "a deleted session should take the threads in its panes"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn deleted_session_takes_its_ended_threads() -> Result<(), Report<StoreError>> {
        // Given a thread whose pane ended, out of its session.
        let store = Store::open_in_memory()?;
        let project_id =
            store.add_project(Path::new("/tmp/orb"), "orb", ProjectKind::Normal, 500)?;
        let inserted = insert_thread(&store, project_id)?;
        let mut row = store
            .load()?
            .1
            .into_iter()
            .find(|row| row.id == inserted.thread)
            .ok_or_else(|| Report::new(StoreError))?;
        row.pane_id = None;
        store.save_thread(&row)?;

        // When deleting the session.
        store.delete_session(inserted.session)?;

        // Then the ended thread is gone too.
        let (_, threads, ..) = store.load()?;
        assert!(
            threads.is_empty(),
            "a deleted session should take the threads that ended in it"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn inserted_thread_loads_its_session() -> Result<(), Report<StoreError>> {
        // Given a project.
        let store = Store::open_in_memory()?;
        let project_id =
            store.add_project(Path::new("/tmp/orb"), "orb", ProjectKind::Normal, 500)?;

        // When inserting a thread.
        let inserted = insert_thread(&store, project_id)?;

        // Then it loads back in its new session.
        let sessions: Vec<Option<SessionId>> = store
            .load()?
            .1
            .into_iter()
            .map(|row| row.orb_session)
            .collect();
        assert_eq!(
            sessions,
            vec![Some(inserted.session)],
            "a new thread should know its session"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn ended_thread_keeps_its_session() -> Result<(), Report<StoreError>> {
        // Given a thread in its session.
        let store = Store::open_in_memory()?;
        let project_id =
            store.add_project(Path::new("/tmp/orb"), "orb", ProjectKind::Normal, 500)?;
        let inserted = insert_thread(&store, project_id)?;
        let mut row = store
            .load()?
            .1
            .into_iter()
            .find(|row| row.id == inserted.thread)
            .ok_or_else(|| Report::new(StoreError))?;

        // When its pane ends.
        row.pane_id = None;
        store.save_thread(&row)?;

        // Then it still loads with that session.
        let sessions: Vec<Option<SessionId>> = store
            .load()?
            .1
            .into_iter()
            .map(|row| row.orb_session)
            .collect();
        assert_eq!(
            sessions,
            vec![Some(inserted.session)],
            "an ended thread should keep its session"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn pane_thread_loads_the_session_of_its_pane() -> Result<(), Report<StoreError>> {
        // Given a pane in a session of the orb project.
        let store = Store::open_in_memory()?;
        let project_id =
            store.add_project(Path::new("/tmp/orb"), "orb", ProjectKind::Normal, 500)?;
        let inserted = insert_thread(&store, project_id)?;
        let pane = store.insert_pane(inserted.session, Path::new("/tmp/orb"))?;

        // When a report starts a thread in that pane.
        let (thread, _) = store.insert_pane_thread(&NewPaneThread {
            pane,
            session_id: "s-new".to_owned(),
            transcript_path: None,
            harness: HarnessId::new("claude"),
            cwd: PathBuf::from("/tmp/orb"),
            created_at: 2_000,
        })?;

        // Then it loads with the pane's session.
        let session = store
            .load()?
            .1
            .into_iter()
            .find(|row| row.id == thread)
            .and_then(|row| row.orb_session);
        assert_eq!(
            session,
            Some(inserted.session),
            "a pane's thread should know the pane's session"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn inserted_threads_session_loads_its_project_and_creation() -> Result<(), Report<StoreError>> {
        // Given a thread inserted at 1000 in project orb.
        let store = Store::open_in_memory()?;
        let project_id =
            store.add_project(Path::new("/tmp/orb"), "orb", ProjectKind::Normal, 500)?;
        insert_thread(&store, project_id)?;

        // When loading the sessions.
        let found: Vec<(ProjectId, SessionKind, i64)> = store
            .layouts()?
            .sessions
            .into_iter()
            .map(|row| (row.project_id, row.kind, row.created_at))
            .collect();

        // Then its session is a plain one of that project, made at 1000.
        assert_eq!(
            found,
            [(project_id, SessionKind::Plain, 1_000)],
            "a session loads its project, kind and creation time"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn inserted_pane_loads_back_in_its_session() -> Result<(), Report<StoreError>> {
        // Given a thread's session.
        let store = Store::open_in_memory()?;
        let project_id =
            store.add_project(Path::new("/tmp/orb"), "orb", ProjectKind::Normal, 500)?;
        let inserted = insert_thread(&store, project_id)?;

        // When inserting a pane in /tmp/logs.
        let pane = store.insert_pane(inserted.session, Path::new("/tmp/logs"))?;

        // Then it loads back in that session and directory.
        let found: Vec<(SessionId, PathBuf)> = store
            .layouts()?
            .panes
            .into_iter()
            .filter(|row| row.id == pane)
            .map(|row| (row.session_id, row.cwd))
            .collect();
        assert_eq!(
            found,
            vec![(inserted.session, PathBuf::from("/tmp/logs"))],
            "the new pane should load back"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn save_thread_saves_its_pane() -> Result<(), Report<StoreError>> {
        // Given a thread and a second pane in its session.
        let store = Store::open_in_memory()?;
        let project_id =
            store.add_project(Path::new("/tmp/orb"), "orb", ProjectKind::Normal, 500)?;
        let inserted = insert_thread(&store, project_id)?;
        let pane = store.insert_pane(inserted.session, Path::new("/tmp/orb"))?;
        let mut row = store
            .load()?
            .1
            .into_iter()
            .find(|row| row.id == inserted.thread)
            .ok_or_else(|| Report::new(StoreError))?;

        // When saving the thread in that pane.
        row.pane_id = Some(pane);
        store.save_thread(&row)?;

        // Then it loads back in that pane.
        let panes: Vec<Option<PaneId>> =
            store.load()?.1.into_iter().map(|row| row.pane_id).collect();
        assert_eq!(panes, vec![Some(pane)], "the thread's pane should be saved");
        Ok(())
    }

    #[rstest::rstest]
    fn insert_pane_thread_joins_the_project_of_the_panes_session() -> Result<(), Report<StoreError>>
    {
        // Given a pane in a session of the orb project.
        let store = Store::open_in_memory()?;
        let project_id =
            store.add_project(Path::new("/tmp/orb"), "orb", ProjectKind::Normal, 500)?;
        let inserted = insert_thread(&store, project_id)?;
        let pane = store.insert_pane(inserted.session, Path::new("/tmp/orb"))?;

        // When a report starts a thread in that pane.
        let (thread, project) = store.insert_pane_thread(&NewPaneThread {
            pane,
            session_id: "s-new".to_owned(),
            transcript_path: Some(PathBuf::from("/t/s-new.jsonl")),
            harness: HarnessId::new("claude"),
            cwd: PathBuf::from("/tmp/orb"),
            created_at: 2_000,
        })?;

        // Then it is saved in the orb project, running in that pane.
        let found = store
            .load()?
            .1
            .into_iter()
            .find(|row| row.id == thread)
            .map(|row| (row.project_id, row.pane_id, row.session_id));
        assert_eq!(
            (project, found),
            (
                project_id,
                Some((project_id, Some(pane), Some("s-new".to_owned())))
            ),
            "the thread should join the pane's project and run in the pane"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn set_pane_resume_is_loaded_back_with_the_pane() -> Result<(), Report<StoreError>> {
        // Given a thread's pane.
        let store = Store::open_in_memory()?;
        let project_id =
            store.add_project(Path::new("/tmp/orb"), "orb", ProjectKind::Normal, 500)?;
        let inserted = insert_thread(&store, project_id)?;

        // When setting its resume command.
        store.set_pane_resume(inserted.pane, Some("claude --resume s-1"))?;

        // Then it loads back with the pane.
        let resume: Vec<Option<String>> = store
            .layouts()?
            .panes
            .into_iter()
            .filter(|row| row.id == inserted.pane)
            .map(|row| row.resume)
            .collect();
        assert_eq!(
            resume,
            vec![Some("claude --resume s-1".to_owned())],
            "the resume command should be saved"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn cleared_bg_marker_loads_back_empty() -> Result<(), Report<StoreError>> {
        // Given a migrated v10 database with a Claude pane still to stop.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let path = migrated_v10(dir.path())?;
        let pane = PaneId(pane_of(&path, 3)?);
        let store = Store::open(&path)?;

        // When clearing its marker.
        store.clear_migrated_bg(&[pane])?;

        // Then it loads back without one.
        let marker: Vec<Option<String>> = store
            .layouts()?
            .panes
            .into_iter()
            .filter(|row| row.id == pane)
            .map(|row| row.migrated_bg)
            .collect();
        assert_eq!(marker, vec![None], "a cleared marker should stay cleared");
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
