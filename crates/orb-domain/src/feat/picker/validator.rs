//! Checks whether the open picker's keys can proceed: `⏎` picking a project
//! or adding a directory, `Tab` opening one, `<C-x>` removing a project, and
//! `⏎` picking a thread in the session picker.

use crate::feat::picker::list::PickerItem;

use wherror::Error;

use crate::AppState;
use crate::feat::picker::state::PickerKind;
use crate::feat::sessions::state::ThreadStatus;

/// Why picking a project can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum PickProjectError {
    /// No project picker is open.
    NoPicker,
    /// No project is highlighted.
    NoProject,
}

/// Allow opening the highlighted project's draft, even while a session
/// starts.
///
/// # Errors
///
/// Returns [`PickProjectError::NoPicker`] unless the project picker is open,
/// and [`PickProjectError::NoProject`] when nothing is highlighted.
pub fn validate_pick_project(state: &AppState) -> Result<(), PickProjectError> {
    match &state.picker {
        Some(picker) if *picker.kind() == PickerKind::Projects => match picker.selected() {
            None => Err(PickProjectError::NoProject),
            Some(_) => Ok(()),
        },
        _ => Err(PickProjectError::NoPicker),
    }
}

/// Why opening a directory can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum OpenDirectoryError {
    /// No directory picker is open.
    NoPicker,
    /// No directory is highlighted.
    NoDirectory,
}

/// Allow browsing into the highlighted directory.
///
/// # Errors
///
/// Returns [`OpenDirectoryError::NoPicker`] unless the directory picker is
/// open, and [`OpenDirectoryError::NoDirectory`] when nothing is highlighted.
pub fn validate_open_directory(state: &AppState) -> Result<(), OpenDirectoryError> {
    match &state.picker {
        Some(picker) if matches!(picker.kind(), PickerKind::Directories { .. }) => {
            match picker.selected() {
                None => Err(OpenDirectoryError::NoDirectory),
                Some(_) => Ok(()),
            }
        }
        _ => Err(OpenDirectoryError::NoPicker),
    }
}

/// Why adding a directory as a project can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum AddDirectoryError {
    /// No directory picker is open.
    NoPicker,
    /// Nothing is highlighted, and the typed path doesn't end in `/`.
    NothingToAdd,
}

/// Allow adding the highlighted directory, or the typed one.
///
/// # Errors
///
/// Returns [`AddDirectoryError::NoPicker`] unless the directory picker is
/// open, and [`AddDirectoryError::NothingToAdd`] when it has no directory to
/// add.
pub fn validate_add_directory(state: &AppState) -> Result<(), AddDirectoryError> {
    match &state.picker {
        Some(picker) if matches!(picker.kind(), PickerKind::Directories { .. }) => {
            match picker.directory_to_add() {
                None => Err(AddDirectoryError::NothingToAdd),
                Some(_) => Ok(()),
            }
        }
        _ => Err(AddDirectoryError::NoPicker),
    }
}

/// Why removing a project can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum RemoveProjectError {
    /// No project filter picker is open.
    NoPicker,
    /// The highlighted row isn't a project.
    NotAProject,
}

/// Allow asking to remove the project highlighted in the project filter.
///
/// # Errors
///
/// Returns [`RemoveProjectError::NoPicker`] unless the project filter picker
/// is open, and [`RemoveProjectError::NotAProject`] when `All projects`, or
/// nothing, is highlighted.
pub fn validate_remove_project(state: &AppState) -> Result<(), RemoveProjectError> {
    match &state.picker {
        Some(picker) if *picker.kind() == PickerKind::ProjectFilter => match picker.selected() {
            Some(PickerItem::Project { .. }) => Ok(()),
            _ => Err(RemoveProjectError::NotAProject),
        },
        _ => Err(RemoveProjectError::NoPicker),
    }
}

/// Why picking a thread in the session picker can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum PickSessionError {
    /// No session picker is open.
    NoPicker,
    /// No thread is highlighted.
    NoThread,
    /// The thread was deleted, or is being deleted, since the picker opened.
    Deleted,
    /// Claude no longer knows the thread's session.
    Gone,
}

/// Allow jumping into the thread highlighted in the session picker.
///
/// # Errors
///
/// Returns [`PickSessionError::NoPicker`] unless the session picker is open,
/// [`PickSessionError::NoThread`] when nothing is highlighted,
/// [`PickSessionError::Deleted`] when the thread is gone from the sidebar or
/// being deleted, and [`PickSessionError::Gone`] when its session is `Gone`.
pub fn validate_pick_session(state: &AppState) -> Result<(), PickSessionError> {
    let id = match &state.picker {
        Some(picker) if matches!(picker.kind(), PickerKind::Sessions { .. }) => {
            picker.selected_thread().ok_or(PickSessionError::NoThread)?
        }
        _ => return Err(PickSessionError::NoPicker),
    };
    if state.sessions.deleting.contains(&id) {
        return Err(PickSessionError::Deleted);
    }
    match state.sessions.threads().find(|thread| thread.id == id) {
        None => Err(PickSessionError::Deleted),
        Some(thread) if thread.status == ThreadStatus::Gone => Err(PickSessionError::Gone),
        Some(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{
        AddDirectoryError, OpenDirectoryError, PickProjectError, PickSessionError,
        RemoveProjectError, validate_add_directory, validate_open_directory, validate_pick_project,
        validate_pick_session, validate_remove_project,
    };
    use crate::feat::picker::list::PickerItem;
    use crate::feat::picker::state::PickerState;
    use crate::feat::sessions::state::{ProjectId, ProjectKind, Sessions, ThreadId};
    use crate::{AppState, Focus};

    const HOME: &str = "/home/me";

    fn alpha() -> PickerItem {
        PickerItem::Project {
            id: ProjectId(1),
            title: "alpha".into(),
            root: "/alpha".into(),
            kind: ProjectKind::Normal,
        }
    }

    /// The session picker over one row, thread 1.
    fn picking_thread_1() -> PickerState {
        PickerState::sessions(
            vec![PickerItem::Thread {
                id: ThreadId(1),
                label: "work/New thread".into(),
                split: 5,
                settled: false,
            }],
            Focus::Sidebar,
        )
    }

    /// The directory picker at `~/`, which lists `names`.
    fn browsing(names: &[&str]) -> PickerState {
        let (mut picker, _) = PickerState::directories(PathBuf::from(HOME), Focus::Sidebar);
        let names = names.iter().map(|name| (*name).to_owned()).collect();
        picker.show_directories(Path::new(HOME), names);
        picker
    }

    #[rstest::rstest]
    fn pick_project_is_refused_without_a_project() {
        // Given an open project picker with no projects.
        let state = AppState {
            picker: Some(PickerState::projects(vec![], Focus::Sidebar)),
            ..AppState::default()
        };

        // When validating a pick.
        let result = validate_pick_project(&state);

        // Then validation fails with NoProject.
        assert_eq!(
            result,
            Err(PickProjectError::NoProject),
            "nothing highlighted can't be picked"
        );
    }

    #[rstest::rstest]
    fn pick_project_is_allowed_while_starting() {
        // Given alpha highlighted while a session start is in flight.
        let state = AppState {
            picker: Some(PickerState::projects(vec![alpha()], Focus::Sidebar)),
            sessions: Sessions {
                starting: true,
                ..Sessions::default()
            },
            ..AppState::default()
        };

        // When validating a pick.
        let result = validate_pick_project(&state);

        // Then it is allowed.
        assert_eq!(
            result,
            Ok(()),
            "a draft can be opened while another session starts"
        );
    }

    #[rstest::rstest]
    fn open_directory_is_refused_without_a_directory() {
        // Given the directory picker at `~/`, which lists nothing.
        let state = AppState {
            picker: Some(browsing(&[])),
            ..AppState::default()
        };

        // When validating an open.
        let result = validate_open_directory(&state);

        // Then validation fails with NoDirectory.
        assert_eq!(
            result,
            Err(OpenDirectoryError::NoDirectory),
            "nothing highlighted can't be opened"
        );
    }

    #[rstest::rstest]
    fn add_directory_is_refused_when_the_filter_matches_nothing() {
        // Given the directory picker at `~/`, which lists `dev`, with `x`
        // typed, which matches nothing.
        let state = AppState {
            picker: Some({
                let mut picker = browsing(&["dev"]);
                picker.insert('x');
                picker
            }),
            ..AppState::default()
        };

        // When validating an add.
        let result = validate_add_directory(&state);

        // Then validation fails with NothingToAdd.
        assert_eq!(
            result,
            Err(AddDirectoryError::NothingToAdd),
            "an unmatched filter has nothing to add"
        );
    }

    #[rstest::rstest]
    fn remove_project_is_refused_on_all_projects() {
        // Given the project filter with All projects highlighted.
        let state = AppState {
            picker: Some(PickerState::project_filter(
                vec![PickerItem::AllProjects, alpha()],
                None,
                Focus::Sidebar,
            )),
            ..AppState::default()
        };

        // When validating a removal.
        let result = validate_remove_project(&state);

        // Then validation fails with NotAProject.
        assert_eq!(
            result,
            Err(RemoveProjectError::NotAProject),
            "All projects can't be removed"
        );
    }

    #[rstest::rstest]
    fn remove_project_is_refused_outside_the_project_filter() {
        // Given the ␣n project picker with alpha highlighted.
        let state = AppState {
            picker: Some(PickerState::projects(vec![alpha()], Focus::Sidebar)),
            ..AppState::default()
        };

        // When validating a removal.
        let result = validate_remove_project(&state);

        // Then validation fails with NoPicker.
        assert_eq!(
            result,
            Err(RemoveProjectError::NoPicker),
            "only the project filter removes projects"
        );
    }

    #[rstest::rstest]
    fn remove_project_is_allowed_on_a_project_row() {
        // Given the project filter with alpha highlighted.
        let state = AppState {
            picker: Some(PickerState::project_filter(
                vec![PickerItem::AllProjects, alpha()],
                Some(ProjectId(1)),
                Focus::Sidebar,
            )),
            ..AppState::default()
        };

        // When validating a removal.
        let result = validate_remove_project(&state);

        // Then it is allowed.
        assert_eq!(result, Ok(()), "a project row can be removed");
    }

    #[rstest::rstest]
    fn pick_session_is_refused_for_a_thread_being_deleted() {
        // Given thread 1 highlighted in the session picker while it's being
        // deleted.
        let state = AppState {
            picker: Some(picking_thread_1()),
            sessions: Sessions {
                deleting: [ThreadId(1)].into(),
                ..Sessions::default()
            },
            ..AppState::default()
        };

        // When validating a pick.
        let result = validate_pick_session(&state);

        // Then validation fails with Deleted.
        assert_eq!(
            result,
            Err(PickSessionError::Deleted),
            "a thread being deleted can't be jumped into"
        );
    }

    #[rstest::rstest]
    fn pick_session_is_refused_for_a_thread_no_longer_listed() {
        // Given thread 1 highlighted in the session picker, but no longer in
        // the sidebar.
        let state = AppState {
            picker: Some(picking_thread_1()),
            ..AppState::default()
        };

        // When validating a pick.
        let result = validate_pick_session(&state);

        // Then validation fails with Deleted.
        assert_eq!(
            result,
            Err(PickSessionError::Deleted),
            "a deleted thread can't be jumped into"
        );
    }
}
