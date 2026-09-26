//! Checks whether the user's sidebar actions can proceed: starting the
//! selected draft or picking its model or permission mode, pinning, settling
//! or deleting the selected thread, discarding the selected draft, and opening
//! or closing the Settled shelf.

use wherror::Error;

use crate::AppState;
use crate::feat::sessions::state::SidebarItem;

/// Why settling or un-settling can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum ToggleSettleError {
    /// The cursor isn't on a thread.
    NoThread,
    /// Claude is running a turn or waiting on the user.
    InProgress,
}

/// Allow settling a thread only between turns; un-settling is always allowed.
///
/// # Errors
///
/// Returns [`ToggleSettleError::NoThread`] without a selected thread, and
/// [`ToggleSettleError::InProgress`] if the selected thread isn't settled and
/// has a turn underway.
pub fn validate_toggle_settle(state: &AppState) -> Result<(), ToggleSettleError> {
    match state.sessions.selected_thread() {
        None => Err(ToggleSettleError::NoThread),
        Some(thread) if thread.settled_at.is_none() && thread.status.in_progress() => {
            Err(ToggleSettleError::InProgress)
        }
        Some(_) => Ok(()),
    }
}

/// Why pinning or unpinning can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum TogglePinError {
    /// The cursor isn't on a thread.
    NoThread,
}

/// Allow pinning or unpinning the selected thread.
///
/// # Errors
///
/// Returns [`TogglePinError::NoThread`] without a selected thread.
pub fn validate_toggle_pin(state: &AppState) -> Result<(), TogglePinError> {
    match state.sessions.selected_thread() {
        None => Err(TogglePinError::NoThread),
        Some(_) => Ok(()),
    }
}

/// Why deleting can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum DeleteError {
    /// The cursor isn't on a thread or a draft.
    NoSelection,
    /// The cursor is on a draft while a session is being started, which may
    /// be starting from it.
    Starting,
}

/// Allow deleting the selected thread, whatever it is doing, and discarding
/// the selected draft between session starts.
///
/// # Errors
///
/// Returns [`DeleteError::NoSelection`] without a selected thread or draft,
/// and [`DeleteError::Starting`] on a draft while a start is in flight.
pub fn validate_delete(state: &AppState) -> Result<(), DeleteError> {
    let sessions = &state.sessions;
    match (sessions.selected_thread(), sessions.selected_draft()) {
        (None, None) => Err(DeleteError::NoSelection),
        (None, Some(_)) if sessions.starting => Err(DeleteError::Starting),
        _ => Ok(()),
    }
}

/// Why starting the selected draft can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum StartDraftError {
    /// The cursor isn't on a draft.
    NoDraft,
    /// A session is already being started.
    Starting,
}

/// Allow starting the selected draft, one start at a time.
///
/// # Errors
///
/// Returns [`StartDraftError::NoDraft`] without a selected draft, and
/// [`StartDraftError::Starting`] while a start is in flight.
pub fn validate_start_draft(state: &AppState) -> Result<(), StartDraftError> {
    match state.sessions.selected_draft() {
        None => Err(StartDraftError::NoDraft),
        Some(_) if state.sessions.starting => Err(StartDraftError::Starting),
        Some(_) => Ok(()),
    }
}

/// Why picking the selected draft's model or permission mode can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum PickSettingError {
    /// The cursor isn't on a draft.
    NoDraft,
    /// A session is being started, maybe from the draft.
    Starting,
}

/// Allow picking the selected draft's model or permission mode between
/// session starts.
///
/// # Errors
///
/// Returns [`PickSettingError::NoDraft`] without a selected draft, and
/// [`PickSettingError::Starting`] while a start is in flight.
pub fn validate_pick_setting(state: &AppState) -> Result<(), PickSettingError> {
    match state.sessions.selected_draft() {
        None => Err(PickSettingError::NoDraft),
        Some(_) if state.sessions.starting => Err(PickSettingError::Starting),
        Some(_) => Ok(()),
    }
}

/// Why opening the Settled shelf can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum OpenShelfError {
    /// The cursor isn't on the shelf's header.
    NotOnShelf,
}

/// Allow opening the Settled shelf from its header.
///
/// # Errors
///
/// Returns [`OpenShelfError::NotOnShelf`] unless the cursor is on the header.
pub fn validate_open_shelf(state: &AppState) -> Result<(), OpenShelfError> {
    match state.sessions.cursor {
        Some(SidebarItem::SettledShelf) => Ok(()),
        _ => Err(OpenShelfError::NotOnShelf),
    }
}

/// Why closing the Settled shelf can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum CloseShelfError {
    /// The cursor is neither on the shelf's header nor in the open shelf.
    NotInShelf,
}

/// Allow closing the Settled shelf from its header or from a settled thread
/// in the open shelf.
///
/// # Errors
///
/// Returns [`CloseShelfError::NotInShelf`] when the cursor is anywhere else.
pub fn validate_close_shelf(state: &AppState) -> Result<(), CloseShelfError> {
    let sessions = &state.sessions;
    match sessions.cursor {
        Some(SidebarItem::SettledShelf) => Ok(()),
        Some(SidebarItem::Thread(_))
            if sessions.shelf_open
                && sessions
                    .selected_thread()
                    .is_some_and(|thread| thread.settled_at.is_some()) =>
        {
            Ok(())
        }
        _ => Err(CloseShelfError::NotInShelf),
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use super::{
        DeleteError, PickSettingError, StartDraftError, ToggleSettleError, validate_delete,
        validate_pick_setting, validate_start_draft, validate_toggle_settle,
    };
    use crate::AppState;
    use crate::feat::sessions::state::{
        Draft, DraftWorkspace, Project, ProjectId, Sessions, SidebarItem, Thread, ThreadId,
        ThreadStatus,
    };

    /// One project whose local draft is selected, with a start in flight if
    /// `starting`.
    fn draft_selected(starting: bool) -> AppState {
        AppState {
            sessions: Sessions {
                projects: vec![Project {
                    id: ProjectId(1),
                    title: "work".into(),
                    root: "/work".into(),
                    created_at: SystemTime::UNIX_EPOCH,
                    draft: Some(Draft {
                        workspace: DraftWorkspace::Local,
                        branch: None,
                        model: None,
                        permission: None,
                        created_at: SystemTime::UNIX_EPOCH,
                    }),
                    threads: vec![],
                }],
                cursor: Some(SidebarItem::Draft(ProjectId(1))),
                starting,
                ..Sessions::default()
            },
            ..AppState::default()
        }
    }

    #[rstest::rstest]
    fn start_draft_is_refused_without_a_draft() {
        // Given no selected draft.
        let state = AppState::default();

        // When validating a draft start.
        let result = validate_start_draft(&state);

        // Then validation fails with NoDraft.
        assert_eq!(
            result,
            Err(StartDraftError::NoDraft),
            "only a draft can be started"
        );
    }

    #[rstest::rstest]
    fn start_draft_is_refused_while_starting() {
        // Given a selected draft while a start is in flight.
        let state = draft_selected(true);

        // When validating a draft start.
        let result = validate_start_draft(&state);

        // Then validation fails with Starting.
        assert_eq!(
            result,
            Err(StartDraftError::Starting),
            "one start at a time"
        );
    }

    #[rstest::rstest]
    fn start_draft_is_allowed_on_a_selected_draft() {
        // Given a selected draft and no start in flight.
        let state = draft_selected(false);

        // When validating a draft start.
        let result = validate_start_draft(&state);

        // Then it is allowed.
        assert_eq!(result, Ok(()), "a selected draft can start");
    }

    #[rstest::rstest]
    fn pick_setting_is_refused_without_a_draft() {
        // Given no selected draft.
        let state = AppState::default();

        // When validating a model or permission pick.
        let result = validate_pick_setting(&state);

        // Then validation fails with NoDraft.
        assert_eq!(
            result,
            Err(PickSettingError::NoDraft),
            "only a draft has a model and permission to pick"
        );
    }

    #[rstest::rstest]
    fn pick_setting_is_refused_while_starting() {
        // Given a selected draft while a start is in flight.
        let state = draft_selected(true);

        // When validating a model or permission pick.
        let result = validate_pick_setting(&state);

        // Then validation fails with Starting.
        assert_eq!(
            result,
            Err(PickSettingError::Starting),
            "a draft can't change while a start may be reading it"
        );
    }

    #[rstest::rstest]
    fn discarding_a_draft_is_refused_while_starting() {
        // Given a selected draft while a start is in flight.
        let state = draft_selected(true);

        // When validating a delete.
        let result = validate_delete(&state);

        // Then validation fails with Starting.
        assert_eq!(
            result,
            Err(DeleteError::Starting),
            "a draft can't go away while a start may be reading it"
        );
    }

    #[rstest::rstest]
    fn discarding_a_draft_is_allowed_between_starts() {
        // Given a selected draft and no start in flight.
        let state = draft_selected(false);

        // When validating a delete.
        let result = validate_delete(&state);

        // Then it is allowed.
        assert_eq!(result, Ok(()), "a selected draft can be discarded");
    }

    #[rstest::rstest]
    fn settle_is_refused_while_working() {
        // Given a selected thread running a turn.
        let state = AppState {
            sessions: Sessions {
                projects: vec![Project {
                    id: ProjectId(1),
                    title: "work".into(),
                    root: "/work".into(),
                    created_at: SystemTime::UNIX_EPOCH,
                    draft: None,
                    threads: vec![Thread {
                        id: ThreadId(1),
                        title: None,
                        cwd: "/work".into(),
                        transcript: None,
                        status: ThreadStatus::Working,
                        turn_started_at: None,
                        attach_argv: vec![],
                        branch: None,
                        pinned_at: None,
                        settled_at: None,
                        active_since: SystemTime::UNIX_EPOCH,
                        last_activity_at: SystemTime::UNIX_EPOCH,
                        unseen: false,
                    }],
                }],
                cursor: Some(SidebarItem::Thread(ThreadId(1))),
                ..Sessions::default()
            },
            ..AppState::default()
        };

        // When validating a settle.
        let result = validate_toggle_settle(&state);

        // Then validation fails with InProgress.
        assert_eq!(
            result,
            Err(ToggleSettleError::InProgress),
            "a working thread can't be settled"
        );
    }
}
