//! orb's saved projects, threads and drafts, kept in SQLite across launches.
//!
//! For each project it keeps the directory its sessions start in and the
//! workspace, model and permission mode its last draft started with. For each
//! thread it keeps the session ids, the titles, the git branch, how far the
//! transcript has been read, when the running turn started, whether it is
//! pinned or settled, when it last had activity and was last visited, and the
//! model and permission mode its session started with. For each project's
//! draft it keeps the session setup the user picked. It also keeps the
//! sidebar's width and project filter. For each project it also keeps its
//! kind (one the user added, or orb's Research or Learn folder), and it keeps
//! each project's groups: their kind, name, directory, branch, pin and settle
//! state, and the session setup of their draft. Each thread keeps the group it
//! belongs to. It also keeps the jump list's rows, oldest first. The schema
//! grows through an ordered list of migrations. Times are milliseconds since
//! the Unix epoch.

use std::path::{Path, PathBuf};

use error_stack::{Report, ResultExt};
use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, params};
use wherror::Error;

use super::state::{
    DraftWorkspace, GroupId, GroupKind, ProjectId, ProjectKind, SidebarItem, ThreadId,
};
use crate::feat::harness::HarnessId;

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
    /// Whether the harness has generated a title for the thread.
    pub ai_titled: bool,
    /// The `--model` its session started with; `None` = the harness's default.
    pub model: Option<String>,
    /// The `--permission-mode` its session started with; `None` = the
    /// harness's default.
    pub permission_mode: Option<String>,
    /// The group the thread belongs to; `None` = a top-level thread.
    pub group_id: Option<GroupId>,
    /// The harness its session runs in.
    pub harness: HarnessId,
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
    pub model: Option<String>,
    pub permission_mode: Option<String>,
    /// The group the thread is born in; `None` = a top-level thread.
    pub group_id: Option<GroupId>,
    /// The harness its session runs in.
    pub harness: HarnessId,
}

/// A saved group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupRow {
    pub id: GroupId,
    pub project_id: ProjectId,
    pub kind: GroupKind,
    /// The group's slug; unique per project and kind.
    pub name: String,
    /// Where its sessions run; `None` for a Feature until its draft starts.
    pub dir: Option<PathBuf>,
    /// Feature: the branch named after the group; else `None`.
    pub branch: Option<String>,
    pub created_at: i64,
    /// When the user pinned the group; `None` = not pinned.
    pub pinned_at: Option<i64>,
    /// Whether the group is settled or kept active; `None` = neither.
    pub settled_override: Option<SettledOverride>,
    /// When the group was settled.
    pub settled_at: Option<i64>,
    /// When the group was last un-settled.
    pub unsettled_at: Option<i64>,
    /// The `--model` for its draft; `None` = the harness's default.
    pub draft_model: Option<String>,
    /// The `--permission-mode` for its draft; `None` = the harness's default.
    pub draft_permission_mode: Option<String>,
    /// The harness its threads start in by default.
    pub harness: HarnessId,
}

/// A group that was just created and isn't saved yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewGroup {
    pub project_id: ProjectId,
    pub kind: GroupKind,
    pub name: String,
    pub dir: Option<PathBuf>,
    pub branch: Option<String>,
    pub created_at: i64,
    pub draft_model: Option<String>,
    pub draft_permission_mode: Option<String>,
    pub harness: HarnessId,
}

/// A saved draft: the session setup picked for a project's next thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftRow {
    /// A project has at most one draft.
    pub project_id: ProjectId,
    pub workspace: DraftWorkspace,
    pub branch: Option<String>,
    /// `None` = the harness's default.
    pub model: Option<String>,
    /// `None` = the harness's default.
    pub permission_mode: Option<String>,
    pub created_at: i64,
    /// The harness its session will run in.
    pub harness: HarnessId,
}

/// Which kind of workspace a project's last draft started in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LastWorkspace {
    /// The project's root.
    Local,
    /// A new worktree.
    NewWorktree,
    /// A worktree that already existed.
    Previous,
}

impl LastWorkspace {
    fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::NewWorktree => "new_worktree",
            Self::Previous => "previous",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "local" => Some(Self::Local),
            "new_worktree" => Some(Self::NewWorktree),
            "previous" => Some(Self::Previous),
            _ => None,
        }
    }
}

/// The settings a project's last draft started with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastUsed {
    /// `None` for a project last started before orb knew harnesses.
    pub harness: Option<HarnessId>,
    pub workspace: LastWorkspace,
    /// `None` = the harness's default.
    pub model: Option<String>,
    /// `None` = the harness's default.
    pub permission_mode: Option<String>,
}

/// How the user last left orb's layout; `None` = never set.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Ui {
    /// The sidebar's width in columns.
    pub sidebar_width: Option<u16>,
    /// The project the sidebar is filtered to.
    pub project_filter: Option<ProjectId>,
}

/// Everything [`Store::load`] returns: projects, threads, drafts, and groups.
pub type Saved = (
    Vec<ProjectRow>,
    Vec<ThreadRow>,
    Vec<DraftRow>,
    Vec<GroupRow>,
);

/// orb's database of projects, threads, drafts, and groups.
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
    "ALTER TABLE threads ADD COLUMN ai_titled INTEGER NOT NULL DEFAULT 0;",
    "
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
",
    "
    ALTER TABLE projects ADD COLUMN removed_at INTEGER;
    CREATE TABLE ui (
      id INTEGER PRIMARY KEY CHECK (id = 1),
      sidebar_width INTEGER,
      project_filter INTEGER REFERENCES projects(id));
",
    "ALTER TABLE threads ADD COLUMN renamed_title TEXT;",
    "
    ALTER TABLE projects ADD COLUMN kind TEXT;
    CREATE TABLE groups (
      id INTEGER PRIMARY KEY, project_id INTEGER NOT NULL REFERENCES projects(id),
      kind TEXT NOT NULL, name TEXT NOT NULL, dir TEXT, branch TEXT,
      created_at INTEGER NOT NULL,
      pinned_at INTEGER, settled_override TEXT, settled_at INTEGER, unsettled_at INTEGER,
      draft_model TEXT, draft_permission_mode TEXT,
      UNIQUE (project_id, kind, name));
    ALTER TABLE threads ADD COLUMN group_id INTEGER REFERENCES groups(id);
",
    "CREATE TABLE jumps (position INTEGER PRIMARY KEY, kind TEXT NOT NULL, item_id INTEGER NOT NULL);",
    "
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

    /// Every saved project (oldest first), thread (newest first), draft, and
    /// group (oldest first).
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
                        branch, pinned_at, settled_override, settled_at, unsettled_at,
                        last_activity_at, last_visited_at, ai_titled, model, permission_mode,
                        renamed_title, group_id, harness
                 FROM threads ORDER BY created_at DESC, id DESC",
                thread_row,
            )
            .attach("failed to load threads")?;
        let drafts = self
            .query(
                "SELECT project_id, workspace, workspace_path, branch, model, permission_mode,
                        created_at, harness
                 FROM drafts ORDER BY project_id",
                draft_row,
            )
            .attach("failed to load drafts")?;
        let groups = self
            .query(
                "SELECT id, project_id, kind, name, dir, branch, created_at, pinned_at,
                        settled_override, settled_at, unsettled_at, draft_model,
                        draft_permission_mode, harness
                 FROM groups ORDER BY id",
                group_row,
            )
            .attach("failed to load groups")?;
        Ok((projects, threads, drafts, groups))
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
                   (project_id, short_id, cwd, created_at, last_activity_at, last_visited_at,
                    model, permission_mode, group_id, harness)
                 VALUES (?1, ?2, ?3, ?4, ?4, ?4, ?5, ?6, ?7, ?8) RETURNING id",
                params![
                    row.project_id.0,
                    row.short_id,
                    utf8(&row.cwd)?,
                    row.created_at,
                    row.model,
                    row.permission_mode,
                    row.group_id.map(|group| group.0),
                    row.harness.as_str(),
                ],
                |row| row.get(0),
            )
            .map(ThreadId)
            .change_context(StoreError)
            .attach("failed to save the thread")
    }

    /// Updates everything about a thread that changes after it's created: its
    /// session and directory (a thread can move to another workspace), titles,
    /// branch, transcript position, turn start, pin and settle state, and
    /// activity and visit stamps. Its model, permission mode and group stay as
    /// inserted.
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
                        branch = ?8, pinned_at = ?9, settled_override = ?10, settled_at = ?11,
                        unsettled_at = ?12, last_activity_at = ?13, last_visited_at = ?14,
                        short_id = ?15, cwd = ?16, ai_titled = ?17, renamed_title = ?18
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
                    row.short_id,
                    utf8(&row.cwd)?,
                    row.ai_titled,
                    row.renamed_title,
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

    /// Saves a new group.
    ///
    /// # Errors
    ///
    /// Returns an error if the directory isn't UTF-8, the project already has
    /// a group of that kind and name, the project doesn't exist, or the
    /// database can't be written.
    pub fn insert_group(&self, row: &NewGroup) -> Result<GroupId, Report<StoreError>> {
        let dir = row.dir.as_deref().map(utf8).transpose()?;
        self.conn
            .query_row(
                "INSERT INTO groups
                   (project_id, kind, name, dir, branch, created_at, draft_model,
                    draft_permission_mode, harness)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) RETURNING id",
                params![
                    row.project_id.0,
                    group_kind_text(row.kind),
                    row.name,
                    dir,
                    row.branch,
                    row.created_at,
                    row.draft_model,
                    row.draft_permission_mode,
                    row.harness.as_str(),
                ],
                |row| row.get(0),
            )
            .map(GroupId)
            .change_context(StoreError)
            .attach("failed to save the group")
    }

    /// Updates everything about a group that changes after it's created: its
    /// directory, branch, pin and settle state, and draft setup. Its project,
    /// kind and name stay as inserted.
    ///
    /// # Errors
    ///
    /// Returns an error if the directory isn't UTF-8 or the database can't be
    /// written.
    pub fn save_group(&self, row: &GroupRow) -> Result<(), Report<StoreError>> {
        let dir = row.dir.as_deref().map(utf8).transpose()?;
        self.conn
            .execute(
                "UPDATE groups SET dir = ?2, pinned_at = ?3, settled_override = ?4,
                        settled_at = ?5, unsettled_at = ?6, draft_model = ?7,
                        draft_permission_mode = ?8, branch = ?9, harness = ?10
                 WHERE id = ?1",
                params![
                    row.id.0,
                    dir,
                    row.pinned_at,
                    row.settled_override.map(SettledOverride::as_str),
                    row.settled_at,
                    row.unsettled_at,
                    row.draft_model,
                    row.draft_permission_mode,
                    row.branch,
                    row.harness.as_str(),
                ],
            )
            .change_context(StoreError)
            .attach("failed to update the group")?;
        Ok(())
    }

    /// Deletes a group. The caller deletes its threads first.
    ///
    /// # Errors
    ///
    /// Returns an error if a thread still belongs to the group or the database
    /// can't be written.
    pub fn delete_group(&self, id: GroupId) -> Result<(), Report<StoreError>> {
        self.conn
            .execute("DELETE FROM groups WHERE id = ?1", params![id.0])
            .change_context(StoreError)
            .attach("failed to delete the group")?;
        Ok(())
    }

    /// Saves the project's draft, replacing the one it had.
    ///
    /// # Errors
    ///
    /// Returns an error if the workspace path isn't UTF-8, the project doesn't
    /// exist, or the database can't be written.
    pub fn save_draft(&self, row: &DraftRow) -> Result<(), Report<StoreError>> {
        let (workspace, path) = match &row.workspace {
            DraftWorkspace::Local => ("local", None),
            DraftWorkspace::NewWorktree => ("new_worktree", None),
            DraftWorkspace::Existing(path) => ("existing", Some(utf8(path)?)),
        };
        self.conn
            .execute(
                "INSERT INTO drafts
                   (project_id, workspace, workspace_path, branch, model, permission_mode,
                    created_at, harness)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT (project_id) DO UPDATE SET
                   workspace = excluded.workspace, workspace_path = excluded.workspace_path,
                   branch = excluded.branch, model = excluded.model,
                   permission_mode = excluded.permission_mode, created_at = excluded.created_at,
                   harness = excluded.harness",
                params![
                    row.project_id.0,
                    workspace,
                    path,
                    row.branch,
                    row.model,
                    row.permission_mode,
                    row.created_at,
                    row.harness.as_str(),
                ],
            )
            .change_context(StoreError)
            .attach("failed to save the draft")?;
        Ok(())
    }

    /// Deletes the project's draft, if it has one.
    ///
    /// # Errors
    ///
    /// Returns an error if the database can't be written.
    pub fn delete_draft(&self, project: ProjectId) -> Result<(), Report<StoreError>> {
        self.conn
            .execute(
                "DELETE FROM drafts WHERE project_id = ?1",
                params![project.0],
            )
            .change_context(StoreError)
            .attach("failed to delete the draft")?;
        Ok(())
    }

    /// Marks the project removed at `now_ms` and deletes its draft, both or
    /// neither. Its threads stay.
    ///
    /// # Errors
    ///
    /// Returns an error if the database can't be written.
    pub fn remove_project(
        &self,
        project: ProjectId,
        now_ms: i64,
    ) -> Result<(), Report<StoreError>> {
        let tx = self
            .conn
            .unchecked_transaction()
            .change_context(StoreError)
            .attach("failed to start removing the project")?;
        tx.execute(
            "UPDATE projects SET removed_at = ?2 WHERE id = ?1",
            params![project.0, now_ms],
        )
        .change_context(StoreError)
        .attach("failed to mark the project removed")?;
        tx.execute(
            "DELETE FROM drafts WHERE project_id = ?1",
            params![project.0],
        )
        .change_context(StoreError)
        .attach("failed to delete the removed project's draft")?;
        tx.commit()
            .change_context(StoreError)
            .attach("failed to commit removing the project")
    }

    /// What the project's last draft started with; `None` if none has.
    ///
    /// # Errors
    ///
    /// Returns an error if the database can't be read.
    pub fn last_used(&self, project: ProjectId) -> Result<Option<LastUsed>, Report<StoreError>> {
        self.conn
            .query_row(
                "SELECT last_workspace, last_model, last_permission_mode, last_harness FROM projects
                 WHERE id = ?1 AND last_used_at IS NOT NULL",
                params![project.0],
                last_used_row,
            )
            .optional()
            .change_context(StoreError)
            .attach("failed to read the project's last-used settings")
    }

    /// What the most recently started draft of any project started with;
    /// `None` if no draft has started.
    ///
    /// # Errors
    ///
    /// Returns an error if the database can't be read.
    pub fn latest_last_used(&self) -> Result<Option<LastUsed>, Report<StoreError>> {
        self.conn
            .query_row(
                "SELECT last_workspace, last_model, last_permission_mode, last_harness FROM projects
                 WHERE last_used_at IS NOT NULL ORDER BY last_used_at DESC, id DESC LIMIT 1",
                [],
                last_used_row,
            )
            .optional()
            .change_context(StoreError)
            .attach("failed to read the latest last-used settings")
    }

    /// Records what the project's draft started with, at `at_ms`.
    ///
    /// # Errors
    ///
    /// Returns an error if the database can't be written.
    pub fn record_last_used(
        &self,
        project: ProjectId,
        used: &LastUsed,
        at_ms: i64,
    ) -> Result<(), Report<StoreError>> {
        self.conn
            .execute(
                "UPDATE projects SET last_workspace = ?2, last_model = ?3,
                        last_permission_mode = ?4, last_used_at = ?5, last_harness = ?6
                 WHERE id = ?1",
                params![
                    project.0,
                    used.workspace.as_str(),
                    used.model,
                    used.permission_mode,
                    at_ms,
                    used.harness.as_ref().map(HarnessId::as_str),
                ],
            )
            .change_context(StoreError)
            .attach("failed to record the project's last-used settings")?;
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

/// How a group's kind is saved.
fn group_kind_text(kind: GroupKind) -> &'static str {
    match kind {
        GroupKind::Feature => "feature",
        GroupKind::Research => "research",
        GroupKind::Learn => "learn",
    }
}

/// A group row; an unknown kind loads as a Feature.
fn group_row(row: &Row<'_>) -> rusqlite::Result<GroupRow> {
    let kind = match row.get::<_, String>(2)?.as_str() {
        "research" => GroupKind::Research,
        "learn" => GroupKind::Learn,
        _ => GroupKind::Feature,
    };
    Ok(GroupRow {
        id: GroupId(row.get(0)?),
        project_id: ProjectId(row.get(1)?),
        kind,
        name: row.get(3)?,
        dir: row.get::<_, Option<String>>(4)?.map(PathBuf::from),
        branch: row.get(5)?,
        created_at: row.get(6)?,
        pinned_at: row.get(7)?,
        settled_override: row
            .get::<_, Option<String>>(8)?
            .as_deref()
            .and_then(SettledOverride::parse),
        settled_at: row.get(9)?,
        unsettled_at: row.get(10)?,
        draft_model: row.get(11)?,
        draft_permission_mode: row.get(12)?,
        harness: HarnessId::new(row.get::<_, String>(13)?),
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
        ai_titled: row.get(18)?,
        model: row.get(19)?,
        permission_mode: row.get(20)?,
        renamed_title: row.get(21)?,
        group_id: row.get::<_, Option<i64>>(22)?.map(GroupId),
        harness: HarnessId::new(row.get::<_, String>(23)?),
    })
}

/// A draft row; an unknown workspace, or `existing` without a path, loads as
/// a local checkout.
fn draft_row(row: &Row<'_>) -> rusqlite::Result<DraftRow> {
    let workspace = match (
        row.get::<_, String>(1)?.as_str(),
        row.get::<_, Option<String>>(2)?,
    ) {
        ("new_worktree", _) => DraftWorkspace::NewWorktree,
        ("existing", Some(path)) => DraftWorkspace::Existing(PathBuf::from(path)),
        _ => DraftWorkspace::Local,
    };
    Ok(DraftRow {
        project_id: ProjectId(row.get(0)?),
        workspace,
        branch: row.get(3)?,
        model: row.get(4)?,
        permission_mode: row.get(5)?,
        created_at: row.get(6)?,
        harness: HarnessId::new(row.get::<_, String>(7)?),
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

/// How a jump-list row is saved: its kind and id; the Settled header isn't.
fn jump_kind(item: SidebarItem) -> Option<(&'static str, i64)> {
    match item {
        SidebarItem::Thread(id) => Some(("thread", id.0)),
        SidebarItem::Draft(id) => Some(("draft", id.0)),
        SidebarItem::Group(id) => Some(("group", id.0)),
        SidebarItem::GroupDraft(id) => Some(("group_draft", id.0)),
        SidebarItem::SettledShelf => None,
    }
}

/// The jump-list row saved as `kind` and `id`; `None` for an unknown kind.
fn jump_item(kind: &str, id: i64) -> Option<SidebarItem> {
    match kind {
        "thread" => Some(SidebarItem::Thread(ThreadId(id))),
        "draft" => Some(SidebarItem::Draft(ProjectId(id))),
        "group" => Some(SidebarItem::Group(GroupId(id))),
        "group_draft" => Some(SidebarItem::GroupDraft(GroupId(id))),
        _ => None,
    }
}

/// A project's last-used settings; an unknown workspace loads as local.
fn last_used_row(row: &Row<'_>) -> rusqlite::Result<LastUsed> {
    Ok(LastUsed {
        workspace: row
            .get::<_, Option<String>>(0)?
            .as_deref()
            .and_then(LastWorkspace::parse)
            .unwrap_or(LastWorkspace::Local),
        model: row.get(1)?,
        permission_mode: row.get(2)?,
        harness: row.get::<_, Option<String>>(3)?.map(HarnessId::new),
    })
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

    use super::{
        DraftRow, DraftWorkspace, GroupId, GroupKind, GroupRow, LastUsed, LastWorkspace,
        MIGRATIONS, NewGroup, NewThread, ProjectId, ProjectKind, SettledOverride, SidebarItem,
        Store, StoreError, ThreadId, ThreadRow, Ui,
    };

    fn user_version(path: &Path) -> Result<usize, Report<StoreError>> {
        Connection::open(path)
            .and_then(|conn| conn.pragma_query_value(None, "user_version", |row| row.get(0)))
            .change_context(StoreError)
    }

    fn new_thread(project_id: ProjectId) -> NewThread {
        NewThread {
            harness: HarnessId::new("claude"),
            project_id,
            short_id: "28bf38e2".to_owned(),
            cwd: PathBuf::from("/tmp/orb"),
            created_at: 1_000,
            model: None,
            permission_mode: None,
            group_id: None,
        }
    }

    /// A Feature group `GT-514-login` on its branch, created at 2 s.
    fn new_group(project_id: ProjectId) -> NewGroup {
        NewGroup {
            harness: HarnessId::new("claude"),
            project_id,
            kind: GroupKind::Feature,
            name: "GT-514-login".to_owned(),
            dir: None,
            branch: Some("GT-514-login".to_owned()),
            created_at: 2_000,
            draft_model: Some("opus".to_owned()),
            draft_permission_mode: None,
        }
    }

    /// A local draft on `main` for the project, created at 2 s.
    fn draft(project_id: ProjectId) -> DraftRow {
        DraftRow {
            harness: HarnessId::new("claude"),
            project_id,
            workspace: DraftWorkspace::Local,
            branch: Some("main".to_owned()),
            model: None,
            permission_mode: None,
            created_at: 2_000,
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
        let (_, threads, _, _) = Store::open(&path)?.load()?;

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
                conn.execute_batch(sql).change_context(StoreError)?;
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
        let (_, threads, _, _) = Store::open(&path)?.load()?;

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
                conn.execute_batch(sql).change_context(StoreError)?;
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
        let (_, threads, _, _) = Store::open(&path)?.load()?;

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
                conn.execute_batch(sql).change_context(StoreError)?;
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
        let (projects, threads, drafts, _) = Store::open(&path)?.load()?;

        // Then it's at the latest version, every row survives, and the ui
        // table and removed_at column exist.
        let conn = Connection::open(&path).change_context(StoreError)?;
        let has_schema = [
            "SELECT id, sidebar_width, project_filter FROM ui",
            "SELECT removed_at FROM projects",
        ]
        .iter()
        .all(|sql| conn.prepare(sql).is_ok());
        assert_eq!(
            (
                user_version(&path)?,
                projects.len(),
                threads.len(),
                drafts.len(),
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
                conn.execute_batch(sql).change_context(StoreError)?;
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
                conn.execute_batch(sql).change_context(StoreError)?;
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
            conn.execute_batch(sql).change_context(StoreError)?;
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
        let (_, threads, drafts, groups) = Store::open(&path)?.load()?;

        // Then every thread, draft and group runs in claude.
        let harnesses: Vec<String> = threads
            .iter()
            .map(|row| row.harness.to_string())
            .chain(drafts.iter().map(|row| row.harness.to_string()))
            .chain(groups.iter().map(|row| row.harness.to_string()))
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
            let project_id =
                store.add_project(Path::new("/tmp/orb"), "orb", ProjectKind::Normal, 500)?;
            (project_id, store.insert_thread(&new_thread(project_id))?)
        };

        // When reopening the store and loading.
        let (_, threads, _, _) = Store::open(&path)?.load()?;

        // Then the thread comes back with the fields it was saved with.
        let expected = ThreadRow {
            harness: HarnessId::new("claude"),
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
            ai_titled: false,
            model: None,
            permission_mode: None,
            renamed_title: None,
            group_id: None,
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
    fn inserted_group_loads_back() -> Result<(), Report<StoreError>> {
        // Given a store with a project.
        let store = Store::open_in_memory()?;
        let project_id =
            store.add_project(Path::new("/tmp/orb"), "orb", ProjectKind::Normal, 500)?;

        // When inserting a Feature group.
        let id = store.insert_group(&new_group(project_id))?;

        // Then loading returns it with the fields it was saved with.
        let expected = GroupRow {
            harness: HarnessId::new("claude"),
            id,
            project_id,
            kind: GroupKind::Feature,
            name: "GT-514-login".to_owned(),
            dir: None,
            branch: Some("GT-514-login".to_owned()),
            created_at: 2_000,
            pinned_at: None,
            settled_override: None,
            settled_at: None,
            unsettled_at: None,
            draft_model: Some("opus".to_owned()),
            draft_permission_mode: None,
        };
        assert_eq!(
            store.load()?.3,
            vec![expected],
            "the saved group should load back"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn saved_group_updates_load_back() -> Result<(), Report<StoreError>> {
        // Given a store with one group.
        let store = Store::open_in_memory()?;
        let project_id =
            store.add_project(Path::new("/tmp/orb"), "orb", ProjectKind::Normal, 500)?;
        store.insert_group(&new_group(project_id))?;
        let inserted = store
            .load()?
            .3
            .pop()
            .ok_or_else(|| Report::new(StoreError).attach("the group wasn't saved"))?;

        // When saving its directory, branch, pin, settle state, and draft
        // setup.
        let updated = GroupRow {
            dir: Some(PathBuf::from("/wt/orb-GT-514-login")),
            branch: Some("GT-514-login-v2".to_owned()),
            pinned_at: Some(3_000),
            settled_override: Some(SettledOverride::Settled),
            settled_at: Some(4_000),
            unsettled_at: Some(3_500),
            draft_model: Some("sonnet".to_owned()),
            draft_permission_mode: Some("plan".to_owned()),
            ..inserted
        };
        store.save_group(&updated)?;

        // Then loading returns the updated values.
        assert_eq!(
            store.load()?.3,
            vec![updated],
            "the group updates should load back"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn deleted_group_is_gone_after_reload() -> Result<(), Report<StoreError>> {
        // Given a store with one group.
        let store = Store::open_in_memory()?;
        let project_id =
            store.add_project(Path::new("/tmp/orb"), "orb", ProjectKind::Normal, 500)?;
        let id = store.insert_group(&new_group(project_id))?;

        // When deleting it.
        store.delete_group(id)?;

        // Then loading no longer returns it.
        assert!(
            store.load()?.3.is_empty(),
            "the deleted group should not load"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn inserted_thread_loads_back_with_its_group() -> Result<(), Report<StoreError>> {
        // Given a store with a project that has a group.
        let store = Store::open_in_memory()?;
        let project_id =
            store.add_project(Path::new("/tmp/orb"), "orb", ProjectKind::Normal, 500)?;
        let group: GroupId = store.insert_group(&new_group(project_id))?;

        // When inserting a thread in the group.
        store.insert_thread(&NewThread {
            group_id: Some(group),
            ..new_thread(project_id)
        })?;

        // Then the thread loads back in that group.
        let groups: Vec<Option<GroupId>> = store
            .load()?
            .1
            .into_iter()
            .map(|row| row.group_id)
            .collect();
        assert_eq!(
            groups,
            vec![Some(group)],
            "the thread's group should load back"
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
    fn removing_a_project_deletes_its_draft() -> Result<(), Report<StoreError>> {
        // Given a store with a project that has a draft.
        let store = Store::open_in_memory()?;
        let project_id =
            store.add_project(Path::new("/tmp/orb"), "orb", ProjectKind::Normal, 500)?;
        store.save_draft(&draft(project_id))?;

        // When removing the project.
        store.remove_project(project_id, 2_000)?;

        // Then its draft no longer loads.
        assert!(
            store.load()?.2.is_empty(),
            "a removed project's draft should be deleted"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn removing_a_project_keeps_its_threads() -> Result<(), Report<StoreError>> {
        // Given a store with a project that has a thread.
        let store = Store::open_in_memory()?;
        let project_id =
            store.add_project(Path::new("/tmp/orb"), "orb", ProjectKind::Normal, 500)?;
        store.insert_thread(&new_thread(project_id))?;

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
        let thread_id = store.insert_thread(&new_thread(project_id))?;

        // When saving its session id, titles, transcript cursor, and turn start.
        let updated = ThreadRow {
            harness: HarnessId::new("claude"),
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
            ai_titled: false,
            model: None,
            permission_mode: None,
            renamed_title: None,
            group_id: None,
        };
        store.save_thread(&updated)?;

        // Then loading returns the updated values.
        let (_, threads, _, _) = store.load()?;
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
        let project_id =
            store.add_project(Path::new("/tmp/orb"), "orb", ProjectKind::Normal, 500)?;
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
            ai_titled: false,
            ..inserted
        };
        store.save_thread(&updated)?;

        // Then loading returns them.
        let (_, threads, _, _) = store.load()?;
        assert_eq!(threads, vec![updated], "the settle fields should load back");
        Ok(())
    }

    #[rstest::rstest]
    fn renamed_title_loads_back_after_saving() -> Result<(), Report<StoreError>> {
        // Given a store with one thread.
        let store = Store::open_in_memory()?;
        let project_id =
            store.add_project(Path::new("/tmp/orb"), "orb", ProjectKind::Normal, 500)?;
        store.insert_thread(&new_thread(project_id))?;
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
        let thread_id = store.insert_thread(&new_thread(project_id))?;

        // When deleting it.
        store.delete_thread(thread_id)?;

        // Then loading no longer returns it.
        let (_, threads, _, _) = store.load()?;
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
        store.insert_thread(&new_thread(project_id))?;

        // Then its last activity and last visit are its creation time.
        let (_, threads, _, _) = store.load()?;
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
    fn thread_model_and_permission_mode_load_back() -> Result<(), Report<StoreError>> {
        // Given a store with a project.
        let store = Store::open_in_memory()?;
        let project_id =
            store.add_project(Path::new("/tmp/orb"), "orb", ProjectKind::Normal, 500)?;

        // When inserting a thread started with sonnet in plan mode.
        store.insert_thread(&NewThread {
            model: Some("sonnet".to_owned()),
            permission_mode: Some("plan".to_owned()),
            ..new_thread(project_id)
        })?;

        // Then loading returns its model and permission mode.
        let flags: Vec<(Option<String>, Option<String>)> = store
            .load()?
            .1
            .into_iter()
            .map(|row| (row.model, row.permission_mode))
            .collect();
        assert_eq!(
            flags,
            vec![(Some("sonnet".to_owned()), Some("plan".to_owned()))],
            "a thread's model and permission mode should load back"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[case(DraftWorkspace::Local)]
    #[case(DraftWorkspace::NewWorktree)]
    #[case(DraftWorkspace::Existing(PathBuf::from("/wt/orb-0a1b2c3d")))]
    fn saved_draft_loads_back(#[case] workspace: DraftWorkspace) -> Result<(), Report<StoreError>> {
        // Given a store with a project.
        let store = Store::open_in_memory()?;
        let project_id =
            store.add_project(Path::new("/tmp/orb"), "orb", ProjectKind::Normal, 500)?;

        // When saving a draft in the workspace.
        let draft = DraftRow {
            workspace,
            ..draft(project_id)
        };
        store.save_draft(&draft)?;

        // Then loading returns it.
        assert_eq!(store.load()?.2, vec![draft], "the draft should load back");
        Ok(())
    }

    #[rstest::rstest]
    fn saving_a_draft_again_replaces_it() -> Result<(), Report<StoreError>> {
        // Given a store with a project that has a local draft.
        let store = Store::open_in_memory()?;
        let project_id =
            store.add_project(Path::new("/tmp/orb"), "orb", ProjectKind::Normal, 500)?;
        store.save_draft(&draft(project_id))?;

        // When saving the project's draft with a new worktree and opus.
        let replaced = DraftRow {
            workspace: DraftWorkspace::NewWorktree,
            model: Some("opus".to_owned()),
            ..draft(project_id)
        };
        store.save_draft(&replaced)?;

        // Then the project has one draft, the second one.
        assert_eq!(
            store.load()?.2,
            vec![replaced],
            "a project should keep one draft, the latest saved"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn deleted_draft_is_gone_after_reload() -> Result<(), Report<StoreError>> {
        // Given a store with a project that has a draft.
        let store = Store::open_in_memory()?;
        let project_id =
            store.add_project(Path::new("/tmp/orb"), "orb", ProjectKind::Normal, 500)?;
        store.save_draft(&draft(project_id))?;

        // When deleting the project's draft.
        store.delete_draft(project_id)?;

        // Then loading no longer returns it.
        assert!(
            store.load()?.2.is_empty(),
            "the deleted draft should not load"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn recorded_last_used_reads_back() -> Result<(), Report<StoreError>> {
        // Given a store with a project.
        let store = Store::open_in_memory()?;
        let project_id =
            store.add_project(Path::new("/tmp/orb"), "orb", ProjectKind::Normal, 500)?;

        // When recording a new-worktree start with Default model in plan mode.
        let used = LastUsed {
            harness: Some(HarnessId::new("claude")),
            workspace: LastWorkspace::NewWorktree,
            model: None,
            permission_mode: Some("plan".to_owned()),
        };
        store.record_last_used(project_id, &used, 2_000)?;

        // Then the project's last-used settings are those.
        assert_eq!(
            store.last_used(project_id)?,
            Some(used),
            "the recorded settings should read back"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn unused_project_has_no_last_used() -> Result<(), Report<StoreError>> {
        // Given a store with a project no draft has started in.
        let store = Store::open_in_memory()?;
        let project_id =
            store.add_project(Path::new("/tmp/orb"), "orb", ProjectKind::Normal, 500)?;

        // When reading its last-used settings.
        let used = store.last_used(project_id)?;

        // Then there are none.
        assert_eq!(used, None, "a never-used project should have no last-used");
        Ok(())
    }

    #[rstest::rstest]
    fn latest_last_used_is_the_newest_projects() -> Result<(), Report<StoreError>> {
        // Given project a used at 3 s with opus and project b used at 2 s with
        // sonnet.
        let store = Store::open_in_memory()?;
        let a = store.add_project(Path::new("/tmp/a"), "a", ProjectKind::Normal, 500)?;
        let b = store.add_project(Path::new("/tmp/b"), "b", ProjectKind::Normal, 500)?;
        let used = |model: &str| LastUsed {
            harness: Some(HarnessId::new("claude")),
            workspace: LastWorkspace::Local,
            model: Some(model.to_owned()),
            permission_mode: None,
        };
        store.record_last_used(a, &used("opus"), 3_000)?;
        store.record_last_used(b, &used("sonnet"), 2_000)?;

        // When reading the latest last-used settings.
        let latest = store.latest_last_used()?;

        // Then they are project a's.
        assert_eq!(
            latest,
            Some(used("opus")),
            "the most recently used project should win"
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
    fn saved_jumps_read_back() -> Result<(), Report<StoreError>> {
        // Given a fresh store.
        let store = Store::open_in_memory()?;

        // When saving a jump list of every kind of row.
        let jumps = vec![
            SidebarItem::Thread(ThreadId(3)),
            SidebarItem::GroupDraft(GroupId(9)),
            SidebarItem::Draft(ProjectId(1)),
            SidebarItem::Group(GroupId(9)),
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
    fn saving_jumps_again_replaces_them() -> Result<(), Report<StoreError>> {
        // Given a store with threads 1 and 2 saved as the jump list.
        let store = Store::open_in_memory()?;
        store.save_jumps(&[
            SidebarItem::Thread(ThreadId(1)),
            SidebarItem::Thread(ThreadId(2)),
        ])?;

        // When saving thread 3 alone.
        let jumps = vec![SidebarItem::Thread(ThreadId(3))];
        store.save_jumps(&jumps)?;

        // Then only thread 3 reads back.
        assert_eq!(
            store.jumps()?,
            jumps,
            "the latest jump list should replace the old"
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
