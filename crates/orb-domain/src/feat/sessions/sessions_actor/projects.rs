//! Adding and removing projects, and Research and Learn folder sessions.

use super::*;

impl SessionsActor {
    /// Saves `root` as a project, unless it already is one, and shows it; a
    /// removed project is restored.
    pub(super) fn add_project(&mut self, root: PathBuf) {
        let now = now_ms();
        let title = project_title(&root);
        let added = if root.is_dir() {
            self.store
                .add_project(&root, &title, ProjectKind::Normal, now)
                .map_err(|report| save_failed(&self.services.log, &report))
        } else {
            Err(format!("not a directory: {}", root.display()))
        };
        let repo = added.is_ok() && self.services.git.project_path(&root).is_some();
        match added {
            Ok(id) => {
                let notice = format!("Added project {title}");
                show_project(
                    &mut self.state.write().sessions,
                    id,
                    title,
                    root,
                    ProjectKind::Normal,
                    repo,
                    now,
                );
                self.report.report(Outcome::Recovered);
                self.report.report(Outcome::Notice(notice));
                self.report.report(Outcome::ProjectsChanged);
            }
            Err(error) => self.report.report(Outcome::Failed(error)),
        }
    }

    /// Makes a `kind` session named `name` in orb's own folder for the kind:
    /// adds or restores its project, copies `<orb_root>/templates/<kind>`
    /// (seeded when missing) to `<orb_root>/<kind>/<name>`, refused when that
    /// folder exists, and saves a session of that kind there with one shell,
    /// selected and reported ready to open while `if_active`, the session
    /// active when the user asked, still is. A made session outside the
    /// project filter clears it. The name box closes once the session is made
    /// and otherwise stays open with the reason on the mode line; a folder
    /// this call made is removed again.
    pub(super) fn new_folder_session(
        &mut self,
        kind: FolderKind,
        name: &str,
        if_active: Option<SessionId>,
    ) {
        let now = now_ms();
        let (project_kind, kind_dir, title) = own_folder(kind);
        let root = self.orb_root.join(kind_dir);
        let project = fs::create_dir_all(&root)
            .map_err(|error| failed(&self.services.log, FOLDER_UNMADE, &error))
            .and_then(|()| {
                self.store
                    .add_project(&root, title, project_kind, now)
                    .map_err(|report| save_failed(&self.services.log, &report))
            });
        if let Ok(id) = project {
            let mut app = self.state.write();
            show_project(
                &mut app.sessions,
                id,
                title.to_owned(),
                root.clone(),
                project_kind,
                false,
                now,
            );
        }
        let made = project.and_then(|id| {
            let dir = self.make_folder(kind, kind_dir, &root, name)?;
            let saved = self.save_session(id, session_kind(project_kind), &dir, None, None);
            if saved.is_err() {
                remove_unstarted(&self.services.log, &dir);
            }
            saved.map(|session| (id, session))
        });
        let unfiltered = {
            let mut app = self.state.write();
            let app = &mut *app;
            answer_name_box(app, made.is_ok());
            let sessions = &mut app.sessions;
            let outside = made
                .as_ref()
                .is_ok_and(|&(id, _)| sessions.filter.is_some_and(|filter| filter != id));
            if outside {
                sessions.filter = None;
            }
            outside
        };
        if unfiltered {
            self.save_ui();
        }
        let opens = made
            .as_ref()
            .ok()
            .map(|&(_, session)| Opens { session, if_active });
        self.end_start(made.map(drop), opens);
    }

    /// Copies `kind`'s template (`<orb_root>/templates/<kind_dir>`, seeded
    /// when missing) to `root/name` and returns that folder; refused when it
    /// exists. A folder this call made is removed again on failure. The
    /// error is the message box text.
    fn make_folder(
        &self,
        kind: FolderKind,
        kind_dir: &str,
        root: &Path,
        name: &str,
    ) -> Result<PathBuf, String> {
        let dir = root.join(name);
        if dir.exists() {
            return Err(folder_exists(kind, name));
        }
        let template = self.orb_root.join("templates").join(kind_dir);
        let seeded = if template.exists() {
            Ok(())
        } else {
            template::seed(&template, kind)
        };
        match seeded.and_then(|()| template::copy(&template, &dir)) {
            Ok(()) => Ok(dir),
            Err(report) => {
                let shown = failed(&self.services.log, FOLDER_UNMADE, &report);
                remove_unstarted(&self.services.log, &dir);
                Err(shown)
            }
        }
    }

    /// Removes project `id` from `<C-g> n` and the project filter once the store
    /// has; its sessions stay.
    pub(super) fn remove_project(&mut self, id: ProjectId) {
        if let Err(report) = self.store.remove_project(id, now_ms()) {
            let error = save_failed(&self.services.log, &report);
            return self.report.report(Outcome::Failed(error));
        }
        if let Some(project) = self
            .state
            .write()
            .sessions
            .projects
            .iter_mut()
            .find(|p| p.id == id)
        {
            project.removed = true;
        }
        self.report.report(Outcome::ProjectsChanged);
    }
}

/// Shows project `id` in `sessions`: restores it if removed, taking `kind`
/// unless that's [`ProjectKind::Normal`] (as the store keeps a saved kind),
/// or adds it; either way `repo` says whether it is a git repository.
fn show_project(
    sessions: &mut Sessions,
    id: ProjectId,
    title: String,
    root: PathBuf,
    kind: ProjectKind,
    repo: bool,
    now: i64,
) {
    match sessions
        .projects
        .iter_mut()
        .find(|project| project.id == id)
    {
        Some(project) => {
            project.removed = false;
            project.repo = repo;
            if kind != ProjectKind::Normal {
                project.kind = kind;
            }
        }
        None => sessions.projects.push(Project {
            id,
            title,
            root,
            created_at: from_ms(now),
            agents: Vec::new(),
            repo,
            removed: false,
            kind,
        }),
    }
}

/// orb's own folder for a `kind` session: its project's kind, its folder's
/// name under orb's directory, and its project's title.
fn own_folder(kind: FolderKind) -> (ProjectKind, &'static str, &'static str) {
    match kind {
        FolderKind::Research => (ProjectKind::Research, "research", "Research"),
        FolderKind::Learn => (ProjectKind::Learn, "learn", "Learn"),
    }
}

/// A project's title: its directory's name, else the whole path.
pub(super) fn project_title(root: &Path) -> String {
    root.file_name().map_or_else(
        || root.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}
