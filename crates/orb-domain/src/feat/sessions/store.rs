//! orb's saved projects, agents and sessions, kept in SQLite across
//! launches.
//!
//! For each project it keeps the directory its sessions start in. For each
//! agent it keeps the session ids, the titles, the git branch, how far the
//! transcript has been read, when the running turn started, when it last had
//! activity and was last visited, and the model its session started with.
//! It also keeps the sidebar's width and project filter. For each project it
//! also keeps its
//! kind (one the user added, or orb's Research or Learn folder). It also
//! keeps the jump list's rows, oldest first. Each session
//! keeps its directory and its tabs in order, each tab its split tree,
//! focused pane, swap layout and whether it was changed by hand, and each
//! pane its directory, zmx session, name and the command that brings its
//! agent back; an agent keeps the pane it runs in.
//! The schema grows through an ordered list of migrations. Before any
//! upgrade the database is copied aside to `<file>.bak-v<old version>`, and
//! only the three newest copies are kept. A database from a newer orb is
//! refused with the name of the copy to put back, and a test build (one not
//! run from cargo's bin directory) refuses to upgrade unless asked. Times are
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

use super::state::{ProjectKind, SessionKind, SidebarItem};
use crate::common::Log;
use crate::feat::harness::HarnessId;
use crate::feat::layout::floats::{FloatLayout, FloatRect};
use crate::feat::layout::tree::{Node, SwapLayout, TileLayout};
use crate::ids::{AgentId, PaneId, ProjectId, SessionId};

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

/// A saved agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRow {
    pub id: AgentId,
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
    /// When orb last saw a turn end, else when the agent was created.
    pub last_activity_at: i64,
    /// When the user last selected the agent.
    pub last_visited_at: i64,
    /// Whether the harness has generated a title for the agent.
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

/// An agent's settle state as set by the user or by activity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettledOverride {
    /// Settled onto the shelf.
    Settled,
    /// Un-settled by the user, until the next turn activity.
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

/// An agent an agent report started in an existing pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPaneAgent {
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
    /// Where its floats go when laid out.
    pub float_layout: FloatLayout,
    /// Whether its floats were moved or resized by hand since they were last laid out.
    pub floats_hand_changed: bool,
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
    /// Where it is in the swap layout list.
    pub swap_layout: SwapLayout,
    /// Whether it was changed by hand since its last relayout.
    pub hand_changed: bool,
}

/// A saved float of a session.
#[derive(Debug, Clone, PartialEq)]
pub struct FloatRow {
    pub session_id: SessionId,
    /// Its place in the floats, from 0 at the bottom.
    pub position: usize,
    pub pane_id: PaneId,
    /// Its place as shares of the tab body.
    pub rect: FloatRect,
    /// Whether it stays drawn while the floats are hidden.
    pub pinned: bool,
}

/// A session's floats as saved: each float, their layout, and whether they
/// were changed by hand since last laid out.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionFloats {
    pub rows: Vec<FloatRow>,
    pub layout: FloatLayout,
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
    /// The name the user gave it.
    pub name: Option<String>,
}

/// Every saved session, tab, float and pane, for restoring layouts.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SavedLayouts {
    pub sessions: Vec<SessionRow>,
    /// Ordered by session, then position.
    pub tabs: Vec<TabRow>,
    /// Ordered by session, then position.
    pub floats: Vec<FloatRow>,
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

/// Everything [`Store::load`] returns: projects and agents.
pub type Saved = (Vec<ProjectRow>, Vec<AgentRow>);

/// orb's database of projects, agents and sessions.
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
    Migration::Sql("
    CREATE TABLE floats (
      session_id INTEGER NOT NULL REFERENCES sessions(id),
      position INTEGER NOT NULL,
      pane_id INTEGER NOT NULL REFERENCES panes(id),
      x REAL NOT NULL, y REAL NOT NULL, width REAL NOT NULL, height REAL NOT NULL,
      pinned INTEGER NOT NULL DEFAULT 0);
    ALTER TABLE sessions ADD COLUMN float_layout TEXT NOT NULL DEFAULT 'staggered';
    ALTER TABLE sessions ADD COLUMN floats_hand_changed INTEGER NOT NULL DEFAULT 0;
"),
    Migration::Sql("
    CREATE TABLE threads_new (
      id INTEGER PRIMARY KEY, project_id INTEGER NOT NULL REFERENCES projects(id),
      short_id TEXT NOT NULL UNIQUE, session_id TEXT, title TEXT, cwd TEXT NOT NULL,
      transcript_path TEXT, transcript_offset INTEGER NOT NULL DEFAULT 0,
      created_at INTEGER NOT NULL, turn_started_at INTEGER, custom_title TEXT, branch TEXT,
      last_activity_at INTEGER NOT NULL DEFAULT 0, last_visited_at INTEGER NOT NULL DEFAULT 0,
      ai_titled INTEGER NOT NULL DEFAULT 0, model TEXT, renamed_title TEXT,
      harness TEXT NOT NULL DEFAULT 'claude',
      pane_id INTEGER REFERENCES panes(id), orb_session_id INTEGER REFERENCES sessions(id));
    INSERT INTO threads_new (id, project_id, short_id, session_id, title, cwd, transcript_path,
      transcript_offset, created_at, turn_started_at, custom_title, branch, last_activity_at,
      last_visited_at, ai_titled, model, renamed_title, harness, pane_id, orb_session_id)
    SELECT id, project_id, short_id, session_id, title, cwd, transcript_path,
      transcript_offset, created_at, turn_started_at, custom_title, branch, last_activity_at,
      last_visited_at, ai_titled, model, renamed_title, harness, pane_id, orb_session_id
    FROM threads;
    DROP TABLE threads;
    ALTER TABLE threads_new RENAME TO threads;
    DROP TABLE groups;
    DROP TABLE drafts;
    ALTER TABLE projects DROP COLUMN last_workspace;
    ALTER TABLE projects DROP COLUMN last_model;
    ALTER TABLE projects DROP COLUMN last_permission_mode;
    ALTER TABLE projects DROP COLUMN last_used_at;
    ALTER TABLE projects DROP COLUMN last_harness;
    ALTER TABLE panes DROP COLUMN migrated_bg;
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

/// Whether opening the store may upgrade an older database.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Upgrade {
    Allowed,
    /// A test build: refuse to upgrade, so the installed orb can still open it.
    AskFirst,
}

/// The installed orb (run from `cargo_bin`) may upgrade; any other orb asks
/// first unless started with `--upgrade-data` (`upgrade_flag`). An exe whose
/// folder can't be resolved counts as a test build.
#[must_use]
pub fn upgrade_policy(exe: Option<&Path>, cargo_bin: &Path, upgrade_flag: bool) -> Upgrade {
    if upgrade_flag {
        return Upgrade::Allowed;
    }
    let installed = exe
        .and_then(Path::parent)
        .and_then(|dir| dir.canonicalize().ok())
        .zip(cargo_bin.canonicalize().ok())
        .is_some_and(|(dir, bin)| dir == bin);
    if installed {
        Upgrade::Allowed
    } else {
        Upgrade::AskFirst
    }
}

impl Store {
    /// Opens the database at `path` like `open_live` for the installed orb,
    /// logging nowhere.
    ///
    /// # Errors
    ///
    /// As `open_live` with `Upgrade::Allowed`.
    pub fn open(path: &Path) -> Result<Self, Report<StoreError>> {
        Self::open_live(path, Upgrade::Allowed, &Log::default())
    }

    /// Opens the database at `path`, creating it and its directories if
    /// needed, and brings its schema up to date. Before upgrading it copies
    /// the database aside and keeps the three newest copies, writing each copy
    /// and removal to `log`.
    ///
    /// # Errors
    ///
    /// Returns an error if the directories or the database can't be created;
    /// if the database is from a newer orb (naming the copy to put back); if
    /// it needs an upgrade and `upgrade` is `AskFirst`; or if the copy or a
    /// migration fails.
    pub fn open_live(path: &Path, upgrade: Upgrade, log: &Log) -> Result<Self, Report<StoreError>> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .change_context(StoreError)
                .attach_with(|| format!("failed to create {}", parent.display()))?;
        }
        let conn = Connection::open(path)
            .change_context(StoreError)
            .attach_with(|| format!("failed to open {}", path.display()))?;
        let version = schema_version(&conn)?;
        if version > MIGRATIONS.len() {
            return Err(newer_database(path, version));
        }
        if version > 0 && version < MIGRATIONS.len() {
            if upgrade == Upgrade::AskFirst {
                return Err(Report::new(StoreError).attach(format!(
                    "this test build won't upgrade orb's saved data from version {version} \
                     to {}; start it with --upgrade-data to allow it (a copy is kept first)",
                    MIGRATIONS.len()
                )));
            }
            back_up_before_upgrade(&conn, path, version, log)?;
            prune_copies(path, version, log);
        }
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

    /// Every saved project (oldest first) and agent (newest first).
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
        let agents = self
            .query(
                "SELECT id, project_id, short_id, session_id, title, cwd, transcript_path,
                        transcript_offset, created_at, turn_started_at, custom_title,
                        branch, last_activity_at, last_visited_at, ai_titled, model,
                        renamed_title, harness, pane_id, orb_session_id
                 FROM threads ORDER BY created_at DESC, id DESC",
                agent_row,
            )
            .attach("failed to load agents")?;
        Ok((projects, agents))
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
    /// `branch`, last active and visited at `now`, with one shell pane in
    /// `dir` and no tab: the caller saves its layout. Returns the session
    /// and its pane.
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
        let tx = self
            .conn
            .unchecked_transaction()
            .change_context(StoreError)
            .attach("failed to start saving the session")?;
        let session = tx
            .query_row(
                "INSERT INTO sessions
                   (project_id, kind, dir, name, branch, created_at, last_activity_at,
                    last_visited_at, active_tab)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?6, 0) RETURNING id",
                params![
                    project.0,
                    session_kind_text(kind),
                    utf8(dir)?,
                    name,
                    branch,
                    now
                ],
                |row| row.get(0),
            )
            .map(SessionId)
            .change_context(StoreError)
            .attach("failed to save the session")?;
        let pane = self.insert_pane(session, dir)?;
        tx.commit()
            .change_context(StoreError)
            .attach("failed to commit the session")?;
        Ok((session, pane))
    }

    /// Every saved session, its tabs and floats in order, and every pane.
    ///
    /// # Errors
    ///
    /// Returns an error if the database can't be read.
    pub fn layouts(&self) -> Result<SavedLayouts, Report<StoreError>> {
        let sessions = self
            .query(
                "SELECT id, dir, active_tab, pinned_at, settled_override, settled_at,
                        unsettled_at, last_activity_at, project_id, kind, name, branch,
                        created_at, floats_hand_changed, float_layout
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
                        floats_hand_changed: row.get(13)?,
                        float_layout: float_layout(&row.get::<_, String>(14)?),
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
        let floats = self
            .query(
                "SELECT session_id, position, pane_id, x, y, width, height, pinned
                 FROM floats ORDER BY session_id, position",
                |row| {
                    Ok(FloatRow {
                        session_id: SessionId(row.get(0)?),
                        position: row.get(1)?,
                        pane_id: PaneId(row.get(2)?),
                        rect: FloatRect {
                            x: row.get::<_, f64>(3)? as f32,
                            y: row.get::<_, f64>(4)? as f32,
                            width: row.get::<_, f64>(5)? as f32,
                            height: row.get::<_, f64>(6)? as f32,
                        },
                        pinned: row.get(7)?,
                    })
                },
            )
            .attach("failed to load floats")?;
        let panes = self
            .query(
                "SELECT id, session_id, cwd, zmx_name, zmx_dir, resume, name
                 FROM panes ORDER BY id",
                |row| {
                    Ok(PaneRow {
                        id: PaneId(row.get(0)?),
                        session_id: SessionId(row.get(1)?),
                        cwd: PathBuf::from(row.get::<_, String>(2)?),
                        zmx_name: row.get(3)?,
                        zmx_dir: row.get::<_, Option<String>>(4)?.map(PathBuf::from),
                        resume: row.get(5)?,
                        name: row.get(6)?,
                    })
                },
            )
            .attach("failed to load panes")?;
        Ok(SavedLayouts {
            sessions,
            tabs,
            floats,
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

    /// Saves `session`'s tabs and its floats, replacing the ones it had, the
    /// position of its active tab, its float layout and whether its floats
    /// were changed by hand, and the names of `panes`, which are every pane
    /// its tabs and floats hold. Its other panes are deleted, and a
    /// agent running in one of them no longer has a pane. Returns the
    /// deleted panes.
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
        floats: &SessionFloats,
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
            tx.execute("DELETE FROM floats WHERE session_id = ?1", [session.0])?;
            for float in &floats.rows {
                tx.execute(
                    "INSERT INTO floats (session_id, position, pane_id, x, y, width, height,
                                         pinned)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        session.0,
                        float.position,
                        float.pane_id.0,
                        f64::from(float.rect.x),
                        f64::from(float.rect.y),
                        f64::from(float.rect.width),
                        f64::from(float.rect.height),
                        float.pinned
                    ],
                )?;
            }
            tx.execute(
                "UPDATE sessions SET active_tab = ?2, floats_hand_changed = ?3, float_layout = ?4
                 WHERE id = ?1",
                params![
                    session.0,
                    active_tab,
                    floats.hand_changed,
                    float_layout_text(floats.layout)
                ],
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

    /// Deletes `session` with its tabs, floats and panes, and the agents that run
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
            "DELETE FROM floats WHERE session_id = ?1",
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

    /// Saves an agent running in `row.pane`, in the project of that pane's
    /// session, last active and visited when created. Returns it and its
    /// project.
    ///
    /// # Errors
    ///
    /// Returns an error if a path isn't UTF-8, the pane doesn't exist, the id
    /// is already an agent's short id, or the database can't be written.
    pub fn insert_pane_agent(
        &self,
        row: &NewPaneAgent,
    ) -> Result<(AgentId, ProjectId), Report<StoreError>> {
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
                |found| Ok((AgentId(found.get(0)?), ProjectId(found.get(1)?))),
            )
            .change_context(StoreError)
            .attach("failed to save the pane's agent")
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

    /// Updates everything about an agent that changes after it's created: its
    /// session and directory (an agent can move to another workspace), titles,
    /// branch, transcript position, turn start, activity and visit stamps,
    /// and the pane it runs in. Its model stays as inserted.
    ///
    /// # Errors
    ///
    /// Returns an error if the cwd or transcript path isn't UTF-8, the short
    /// id belongs to another agent, or the database can't be written.
    pub fn save_agent(&self, row: &AgentRow) -> Result<(), Report<StoreError>> {
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
            .attach("failed to update the agent")?;
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

    /// The saved jump list's rows, oldest first; a row of an unknown kind, or
    /// an agent row whose pane is gone, is skipped.
    ///
    /// # Errors
    ///
    /// Returns an error if the database can't be read.
    pub fn jumps(&self) -> Result<Vec<SidebarItem>, Report<StoreError>> {
        let rows = self
            .query(
                "SELECT j.kind, j.item_id, p.session_id FROM jumps j
                 LEFT JOIN panes p ON j.kind = 'agent' AND p.id = j.item_id
                 ORDER BY j.position",
                |row| {
                    Ok(jump_item(
                        &row.get::<_, String>(0)?,
                        row.get(1)?,
                        row.get::<_, Option<i64>>(2)?,
                    ))
                },
            )
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

fn schema_version(conn: &Connection) -> Result<usize, Report<StoreError>> {
    conn.pragma_query_value(None, "user_version", |row| row.get(0))
        .change_context(StoreError)
        .attach("failed to read the schema version")
}

/// `<file>.bak-v<version>` beside `path`.
fn copy_path(path: &Path, version: usize) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".bak-v{version}"));
    PathBuf::from(name)
}

/// The refusal for a database at `version`, past what this orb knows,
/// naming the newest copy this orb can open.
fn newer_database(path: &Path, version: usize) -> Report<StoreError> {
    let latest = MIGRATIONS.len();
    let hint = match copies(path).into_iter().find(|(n, _)| *n <= latest) {
        Some((_, copy)) => format!(
            "Quit, then put back {} as {} to use this orb.",
            copy.display(),
            path.display()
        ),
        None => "No copy from before that upgrade was found.".to_owned(),
    };
    Report::new(StoreError).attach(format!(
        "orb's saved data is from a newer orb (version {version}; this orb knows {latest}). {hint}"
    ))
}

/// Copies the database (at `version`) to `<file>.bak-v<version>` beside it
/// before an upgrade, once: an existing copy is kept. The copy is written to
/// a `.tmp` file with `VACUUM INTO` (a consistent snapshot, `user_version`
/// included) and renamed into place, so a half-written copy never counts.
fn back_up_before_upgrade(
    conn: &Connection,
    path: &Path,
    version: usize,
    log: &Log,
) -> Result<(), Report<StoreError>> {
    let backup = copy_path(path, version);
    let upgrading = format!(
        "upgrading {} from version {version} to {}",
        path.display(),
        MIGRATIONS.len()
    );
    if backup.exists() {
        log.line(
            "upgrade",
            &format!("{upgrading}; kept the existing copy {}", backup.display()),
        );
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
        .attach_with(|| format!("failed to move the backup to {}", backup.display()))?;
    log.line(
        "upgrade",
        &format!("{upgrading}; copied it to {}", backup.display()),
    );
    Ok(())
}

/// Every `<file>.bak-v<N>` beside `path`, newest version first. Other names
/// (`.tmp` files included) are never listed; an unreadable folder lists none.
fn copies(path: &Path) -> Vec<(usize, PathBuf)> {
    let (Some(dir), Some(file)) = (path.parent(), path.file_name()) else {
        return Vec::new();
    };
    let prefix = format!("{}.bak-v", file.to_string_lossy());
    let listed = if dir.as_os_str().is_empty() {
        std::fs::read_dir(".")
    } else {
        std::fs::read_dir(dir)
    };
    let mut found: Vec<(usize, PathBuf)> = listed
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name();
            let digits = name.to_str()?.strip_prefix(&prefix)?;
            let all_digits = !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit());
            let version = all_digits.then(|| digits.parse().ok()).flatten()?;
            Some((version, dir.join(&name)))
        })
        .collect();
    found.sort_by_key(|(version, _)| std::cmp::Reverse(*version));
    found
}

/// Removes all but the three newest copies, never the one for `kept` (the
/// version being upgraded from), logging each removal. A failed removal is
/// logged and never stops the upgrade.
fn prune_copies(path: &Path, kept: usize, log: &Log) {
    for (_, old) in copies(path)
        .into_iter()
        .skip(3)
        .filter(|(version, _)| *version != kept)
    {
        match std::fs::remove_file(&old) {
            Ok(()) => log.line("delete", &format!("old copy {}: ok", old.display())),
            Err(error) => log.error(
                &format!("delete old copy {}: failed", old.display()),
                &Report::new(error).change_context(StoreError),
            ),
        }
    }
}

/// Turns every group into a session with a tab for each of its agents,
/// newest first, and every agent outside a group (or in a group that is
/// gone) into a session with one tab. Each agent gets a pane of its own,
/// alone in its tab. Agent ids don't change. Pane
/// ids start above every agent id, since earlier versions named zmx sessions
/// `orb-p<agent id>`. The `groups` and `drafts` tables and the agents' old
/// columns stay until v14 drops them.
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
        let agents = collect(
            conn,
            "SELECT id, cwd, harness, short_id, session_id FROM threads
             WHERE group_id = ?1 ORDER BY created_at DESC, id DESC",
            [group],
            migrated_pane,
        )?;
        let panes: Vec<NewPane> = if agents.is_empty() {
            vec![NewPane::shell(session.dir.clone())]
        } else {
            agents
        };
        let (id, _) = insert_session(conn, &session, &panes)?;
        group_sessions.insert(group, id);
    }
    let mut agent_sessions = HashMap::new();
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
    for (agent, session, pane) in lone {
        let (id, _) = insert_session(conn, &session, &[pane])?;
        agent_sessions.insert(agent, id);
    }
    let grouped = collect(
        conn,
        "SELECT id, group_id FROM threads WHERE group_id IS NOT NULL",
        [],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
    )?;
    for (agent, group) in grouped {
        if let Some(session) = group_sessions.get(&group) {
            agent_sessions.insert(agent, *session);
        }
    }
    rewrite_jumps(conn, &agent_sessions, &group_sessions)
}

/// Points agent and group jump rows at their sessions and deletes the rest
/// (drafts, rows whose agent or group is gone).
fn rewrite_jumps(
    conn: &Connection,
    agent_sessions: &HashMap<i64, SessionId>,
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
            "thread" => agent_sessions.get(&item),
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

/// A pane to create in a new session, and the agent that runs in it.
struct NewPane {
    agent: Option<i64>,
    cwd: String,
    zmx_name: Option<String>,
    zmx_dir: Option<String>,
    resume: Option<String>,
    migrated_bg: Option<String>,
}

impl NewPane {
    /// A pane running a shell in `cwd`, with no agent.
    fn shell(cwd: String) -> Self {
        Self {
            agent: None,
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

/// A migrated agent's pane from the 5 columns starting at `at`: agent id,
/// cwd, harness, short id and session id. A Claude pane starts as a shell
/// that will resume the session, its `--bg` session still to stop; a pi pane
/// keeps the zmx session pi already runs in, under `~/.orb/pi`.
fn migrated_pane_at(row: &Row<'_>, at: usize) -> rusqlite::Result<NewPane> {
    let short_id: String = row.get(at + 3)?;
    let session_id: Option<String> = row.get(at + 4)?;
    let shell = NewPane {
        agent: Some(row.get(at)?),
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
/// active, and points each pane's agent at its pane.
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
        if let Some(agent) = pane.agent {
            conn.execute(
                "UPDATE threads SET pane_id = ?2, orb_session_id = ?3 WHERE id = ?1",
                params![agent, pane_id.0, id.0],
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

fn agent_row(row: &Row<'_>) -> rusqlite::Result<AgentRow> {
    Ok(AgentRow {
        id: AgentId(row.get(0)?),
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

/// How a session's float layout is saved.
fn float_layout_text(layout: FloatLayout) -> &'static str {
    match layout {
        FloatLayout::Staggered => "staggered",
        FloatLayout::Enlarged => "enlarged",
        FloatLayout::Spread => "spread",
    }
}

/// The float layout saved as `text`; anything orb doesn't know is staggered.
fn float_layout(text: &str) -> FloatLayout {
    match text {
        "enlarged" => FloatLayout::Enlarged,
        "spread" => FloatLayout::Spread,
        _ => FloatLayout::Staggered,
    }
}

/// How a jump-list row is saved: its kind and id (an agent by its pane); the
/// Settled header isn't.
fn jump_kind(item: SidebarItem) -> Option<(&'static str, i64)> {
    match item {
        SidebarItem::Session(id) => Some(("session", id.0)),
        SidebarItem::Agent { pane, .. } => Some(("agent", pane.0)),
        SidebarItem::SettledShelf => None,
    }
}

/// The jump-list row saved as `kind` and `id`; an agent row needs its pane's
/// `session`. `None` for an unknown kind or an agent whose pane is gone.
fn jump_item(kind: &str, id: i64, session: Option<i64>) -> Option<SidebarItem> {
    match kind {
        "session" => Some(SidebarItem::Session(SessionId(id))),
        "agent" => Some(SidebarItem::Agent {
            session: SessionId(session?),
            pane: PaneId(id),
        }),
        _ => None,
    }
}

/// Fails the test unless every pane row in `store` is held by a saved tab's
/// tree, a float row, or `unread` (panes of tabs whose tree doesn't read).
#[cfg(test)]
pub(crate) fn assert_every_pane_row_held(store: &Store, unread: &[PaneId]) {
    let saved = store.layouts();
    assert!(saved.is_ok(), "the saved layouts should read: {saved:?}");
    let Ok(saved) = saved else {
        return;
    };
    let held: std::collections::HashSet<PaneId> = saved
        .tabs
        .iter()
        .filter_map(|tab| {
            TileLayout::from_json(&tab.layout, tab.focus_pane.unwrap_or(PaneId(0))).ok()
        })
        .flat_map(|tree| tree.pane_ids())
        .chain(saved.floats.iter().map(|float| float.pane_id))
        .chain(unread.iter().copied())
        .collect();
    let orphans: Vec<PaneId> = saved
        .panes
        .iter()
        .map(|pane| pane.id)
        .filter(|id| !held.contains(id))
        .collect();
    assert!(
        orphans.is_empty(),
        "pane rows no tab, float or unread tab holds: {orphans:?}"
    );
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "tests propagate store failures with `?` and assert on the outcome"
)]
mod tests;
