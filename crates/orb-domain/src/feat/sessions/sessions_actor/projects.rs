//! Adding and removing projects and worlds, and making colonies.

use super::*;
use crate::tilde;

impl SessionsActor {
    /// Saves `root` as a project, unless it already is one, and shows it; a
    /// removed project is restored.
    pub(super) fn add_project(&mut self, root: PathBuf) {
        self.save_project(root, ProjectKind::Normal, "project");
    }

    /// Saves `root` as a world of `kind` named after its folder and shows it,
    /// with an `Added world <name>` notice. Refused when `root` is
    /// Research's, Learn's or Incognito's folder, or when another world has
    /// the name, ignoring case, so `research` hits Research. An existing
    /// project at `root` becomes the world.
    pub(super) fn add_world(&mut self, root: PathBuf, kind: ProjectKind) {
        let refused = {
            let app = self.state.read();
            let projects = &app.sessions.projects;
            let lower = project_title(&root).to_lowercase();
            let own = projects.iter().any(|project| {
                project.root == root
                    && matches!(
                        project.kind,
                        ProjectKind::Research | ProjectKind::Learn | ProjectKind::Incognito
                    )
            });
            let taken = projects.iter().any(|project| {
                !project.removed
                    && project.kind.is_world()
                    && project.root != root
                    && project.title.to_lowercase() == lower
            });
            match (own, taken) {
                (true, _) => Some(format!(
                    "{} is one of orb's own folders",
                    tilde(&root, &app.home)
                )),
                (false, true) => Some(format!("World {} already exists", project_title(&root))),
                (false, false) => None,
            }
        };
        match refused {
            Some(error) => self.report.report(Outcome::Failed(error)),
            None => self.save_project(root, kind, "world"),
        }
    }

    /// Saves `root` as a `kind` project, upserting the kind onto an existing
    /// one and restoring a removed one, shows it, and says `Added <noun>
    /// <name>`.
    fn save_project(&mut self, root: PathBuf, kind: ProjectKind, noun: &str) {
        let now = now_ms();
        let title = project_title(&root);
        let added = if root.is_dir() {
            self.store
                .add_project(&root, &title, kind, now)
                .map_err(|report| save_failed(&self.services.log, &report))
        } else {
            Err(format!("not a directory: {}", root.display()))
        };
        let repo = added.is_ok() && self.services.git.project_path(&root).is_some();
        match added {
            Ok(id) => {
                let notice = format!("Added {noun} {title}");
                show_project(
                    &mut self.state.write().sessions,
                    id,
                    title,
                    root,
                    kind,
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

    /// Makes a colony in world `world`, selected and reported ready to open
    /// while `if_active`, the session active when the user asked, still is.
    /// Shared: in the world's folder, named `name` unless empty, refused when
    /// the folder is gone. Fresh: in a new folder `name` under the world's
    /// colonies folder, copied from its kit (orb's built-in kit seeded when
    /// missing), refused when the folder exists, and removed again when the
    /// session isn't saved. A made colony outside the project filter clears
    /// it. The name box closes once the colony is made and otherwise stays
    /// open with the reason on the mode line.
    pub(super) fn new_colony(
        &mut self,
        world: ProjectId,
        name: &str,
        if_active: Option<SessionId>,
    ) {
        let (dirs, home) = {
            let app = self.state.read();
            let dirs = app
                .sessions
                .project(world)
                .filter(|project| !project.removed)
                .and_then(|project| world_dirs(&self.orb_root, project));
            (dirs, app.home.clone())
        };
        let made = match dirs {
            None => Err("couldn't find that world".to_owned()),
            Some(WorldDirs::Shared(root)) if !root.is_dir() => {
                Err(format!("couldn't find {}", tilde(&root, &home)))
            }
            Some(WorldDirs::Shared(root)) => {
                let named = (!name.is_empty()).then(|| name.to_owned());
                self.save_session(world, SessionKind::Colony, &root, None, named, true)
            }
            Some(WorldDirs::Fresh {
                kit,
                colonies,
                seed,
            }) => self
                .make_folder(&kit, &colonies, seed, name, &home)
                .and_then(|dir| {
                    let saved =
                        self.save_session(world, SessionKind::Colony, &dir, None, None, true);
                    if saved.is_err() {
                        remove_unstarted(&self.services.log, &dir);
                    }
                    saved
                }),
        };
        let unfiltered = {
            let mut app = self.state.write();
            let app = &mut *app;
            answer_name_box(app, made.is_ok());
            let sessions = &mut app.sessions;
            let outside = made.is_ok() && sessions.filter.is_some_and(|filter| filter != world);
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
            .map(|&session| Opens { session, if_active });
        self.end_start(made.map(drop), opens);
    }

    /// Copies `kit` (seeded from `seed` when missing) to `colonies/name`,
    /// making `colonies` first, and returns that folder; refused with
    /// `<dir> already exists` when it exists. A folder this call made is
    /// removed again on failure. The error is the message box text.
    fn make_folder(
        &self,
        kit: &Path,
        colonies: &Path,
        seed: Option<FolderKind>,
        name: &str,
        home: &Path,
    ) -> Result<PathBuf, String> {
        fs::create_dir_all(colonies)
            .map_err(|error| failed(&self.services.log, FOLDER_UNMADE, &error))?;
        let dir = colonies.join(name);
        if dir.exists() {
            return Err(format!("{} already exists", tilde(&dir, home)));
        }
        let seeded = match seed {
            Some(kind) if !kit.exists() => template::seed(kit, kind),
            _ => Ok(()),
        };
        match seeded.and_then(|()| template::copy(kit, &dir)) {
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
pub(super) fn own_folder(kind: FolderKind) -> (ProjectKind, &'static str, &'static str) {
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
