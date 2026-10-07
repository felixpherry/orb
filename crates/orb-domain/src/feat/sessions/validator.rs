//! Checks whether the user's sidebar actions can proceed: making a new
//! session, naming a new Research or Learn session's folder, pinning,
//! settling or deleting the selected session (pin and settle are refused on
//! an agent row), opening or closing the Settled shelf, and making an
//! Incognito session.

use wherror::Error;

use crate::AppState;
use crate::feat::sessions::state::{FolderKind, ProjectKind, SidebarItem, folder_slug};
use crate::feat::sidebar::state::{Rename, RenameTarget};

/// Why making a session in a new folder from the name box can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum NewFolderError {
    /// The box isn't naming a new folder, or the slug is empty.
    Empty,
    /// The slug uses something folder names can't: the char or sequence.
    Invalid(String),
    /// `⏎` already asked for the session, and the sessions actor hasn't
    /// answered yet.
    Creating,
}

/// Characters a folder's slug can't hold anywhere.
const INVALID_CHARS: [char; 8] = ['/', '\\', '~', '^', ':', '?', '*', '['];

/// The mode-line text for a `kind` folder `slug` that is already on disk.
#[must_use]
pub fn folder_exists(kind: FolderKind, slug: &str) -> String {
    match kind {
        FolderKind::Research => format!("~/.orb/research/{slug} already exists"),
        FolderKind::Learn => format!("~/.orb/learn/{slug} already exists"),
    }
}

/// Allow making the session the name box names, once its slug is a usable
/// folder name.
///
/// # Errors
///
/// Returns [`NewFolderError::Empty`] when the box isn't naming a new folder
/// or the slug is empty, [`NewFolderError::Invalid`] with the offending char
/// or sequence when the slug holds one of `/ \ ~ ^ : ? * [`, starts with `-`
/// or `.`, or holds `..`, and [`NewFolderError::Creating`] while the box's
/// session is being made.
pub fn validate_new_folder(state: &AppState) -> Result<(), NewFolderError> {
    let Some(Rename {
        target: RenameTarget::NewFolder(_),
        input,
        creating,
    }) = &state.rename
    else {
        return Err(NewFolderError::Empty);
    };
    if *creating {
        return Err(NewFolderError::Creating);
    }
    let slug = folder_slug(input.text());
    let invalid = slug
        .matches(INVALID_CHARS)
        .next()
        .or_else(|| ["-", "."].into_iter().find(|lead| slug.starts_with(lead)))
        .or_else(|| slug.contains("..").then_some(".."));
    match (slug.is_empty(), invalid) {
        (true, _) => Err(NewFolderError::Empty),
        (false, Some(what)) => Err(NewFolderError::Invalid(what.to_owned())),
        (false, None) => Ok(()),
    }
}

/// Why settling or un-settling can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum ToggleSettleError {
    /// The cursor isn't on a session.
    NoSession,
    /// The cursor is on an agent row; pin and settle are per session.
    OnAgent,
    /// One of the session's agent panes is running a turn or waiting on the
    /// user.
    InProgress,
}

/// What the mode line says when settling is refused because a session is
/// working.
pub const SETTLE_IN_PROGRESS: &str = "Can't settle while a session is working";

/// Allow settling the selected session only while none of its agent panes
/// has a turn underway; un-settling is always allowed.
///
/// # Errors
///
/// Returns [`ToggleSettleError::OnAgent`] with the cursor on an agent row,
/// [`ToggleSettleError::NoSession`] without a selected session, and
/// [`ToggleSettleError::InProgress`] if it isn't settled and one of its
/// agents has a turn underway.
pub fn validate_toggle_settle(state: &AppState) -> Result<(), ToggleSettleError> {
    let sessions = &state.sessions;
    if sessions.selected_agent().is_some() {
        return Err(ToggleSettleError::OnAgent);
    }
    match sessions.selected_session() {
        None => Err(ToggleSettleError::NoSession),
        Some(session)
            if session.settled_at.is_none()
                && sessions
                    .agents(session.id)
                    .into_iter()
                    .any(|thread| thread.status.in_progress()) =>
        {
            Err(ToggleSettleError::InProgress)
        }
        Some(_) => Ok(()),
    }
}

/// Why pinning or unpinning can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum TogglePinError {
    /// The cursor isn't on a session.
    NoSession,
    /// The cursor is on an agent row; pin and settle are per session.
    OnAgent,
}

/// Allow pinning or unpinning the selected session.
///
/// # Errors
///
/// Returns [`TogglePinError::OnAgent`] with the cursor on an agent row, and
/// [`TogglePinError::NoSession`] without a selected session.
pub fn validate_toggle_pin(state: &AppState) -> Result<(), TogglePinError> {
    match (
        state.sessions.selected_agent(),
        state.sessions.selected_session(),
    ) {
        (Some(_), _) => Err(TogglePinError::OnAgent),
        (None, Some(_)) => Ok(()),
        (None, None) => Err(TogglePinError::NoSession),
    }
}

/// Why deleting can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum DeleteError {
    /// The cursor isn't on a session.
    NoSelection,
}

/// Allow deleting the selected session, whatever it is doing.
///
/// # Errors
///
/// Returns [`DeleteError::NoSelection`] without a selected session.
pub fn validate_delete(state: &AppState) -> Result<(), DeleteError> {
    match state.sessions.selected_session() {
        Some(_) => Ok(()),
        None => Err(DeleteError::NoSelection),
    }
}

/// Why making a new session can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum NewSessionError {
    /// A session is already being made, or a workspace changed.
    Starting,
}

/// Allow making a new session, one at a time.
///
/// # Errors
///
/// Returns [`NewSessionError::Starting`] while a start is in flight.
pub fn validate_new_session(state: &AppState) -> Result<(), NewSessionError> {
    if state.sessions.starting {
        Err(NewSessionError::Starting)
    } else {
        Ok(())
    }
}

/// Why making an Incognito session can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum NewIncognitoError {
    /// orb has no Incognito project (it adds one at start), or it was removed.
    NoProject,
}

/// Allow making an Incognito session.
///
/// # Errors
///
/// Returns [`NewIncognitoError::NoProject`] without a non-removed Incognito
/// project.
pub fn validate_new_incognito(state: &AppState) -> Result<(), NewIncognitoError> {
    match state.sessions.own_project(ProjectKind::Incognito) {
        None => Err(NewIncognitoError::NoProject),
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

/// Allow closing the Settled shelf from its header or from a settled session
/// in the open shelf.
///
/// # Errors
///
/// Returns [`CloseShelfError::NotInShelf`] when the cursor is anywhere else.
pub fn validate_close_shelf(state: &AppState) -> Result<(), CloseShelfError> {
    let sessions = &state.sessions;
    match sessions.cursor {
        Some(SidebarItem::SettledShelf) => Ok(()),
        Some(SidebarItem::Session(_))
            if sessions.shelf_open
                && sessions
                    .selected_session()
                    .is_some_and(|session| session.settled_at.is_some()) =>
        {
            Ok(())
        }
        _ => Err(CloseShelfError::NotInShelf),
    }
}

#[cfg(test)]
mod tests {
    use crate::feat::harness::HarnessId;
    use std::time::SystemTime;

    use super::{
        CloseShelfError, DeleteError, NewFolderError, NewIncognitoError, NewSessionError,
        TogglePinError, ToggleSettleError, validate_close_shelf, validate_delete,
        validate_new_folder, validate_new_incognito, validate_new_session, validate_toggle_pin,
        validate_toggle_settle,
    };
    use crate::feat::sessions::state::{
        FolderKind, PaneId, PaneLaunch, Project, ProjectId, ProjectKind, SessionId, Sessions,
        SidebarItem, Thread, ThreadId, ThreadStatus, sessions_for,
    };
    use crate::feat::sidebar::state::{Rename, RenameTarget};
    use crate::{AppState, TextInput};

    #[rstest::rstest]
    fn new_session_rejected_while_starting() {
        // Given a session start in flight.
        let state = AppState {
            sessions: Sessions {
                starting: true,
                ..Sessions::default()
            },
            ..AppState::default()
        };

        // When validating a new session.
        let result = validate_new_session(&state);

        // Then validation fails with Starting.
        assert_eq!(
            result,
            Err(NewSessionError::Starting),
            "one new session at a time"
        );
    }

    #[rstest::rstest]
    fn new_session_allowed_otherwise() {
        // Given no start in flight.
        let state = AppState::default();

        // When validating a new session.
        let result = validate_new_session(&state);

        // Then it is allowed.
        assert_eq!(result, Ok(()), "a new session can start");
    }

    /// orb's Incognito project, `removed` or not, with nothing selected.
    fn incognito_project(removed: bool) -> AppState {
        AppState {
            sessions: Sessions {
                projects: vec![Project {
                    id: ProjectId(1),
                    title: "Incognito".into(),
                    root: "/tmp/orb-incognito".into(),
                    created_at: SystemTime::UNIX_EPOCH,
                    removed,
                    repo: true,
                    threads: vec![],
                    kind: ProjectKind::Incognito,
                }],
                ..Sessions::default()
            },
            ..AppState::default()
        }
    }

    #[rstest::rstest]
    #[case::none(AppState::default())]
    #[case::removed(incognito_project(true))]
    fn new_incognito_rejected_without_an_incognito_project(#[case] state: AppState) {
        // Given no Incognito project, or only a removed one.

        // When validating a new Incognito session.
        let result = validate_new_incognito(&state);

        // Then validation fails with NoProject.
        assert_eq!(
            result,
            Err(NewIncognitoError::NoProject),
            "an Incognito session needs the Incognito project"
        );
    }

    /// One project holding session 1, whose agent thread 1 is `status`, with
    /// the cursor on it.
    fn on_session(status: ThreadStatus) -> AppState {
        let mut state = AppState {
            sessions: Sessions {
                projects: vec![Project {
                    id: ProjectId(1),
                    title: "work".into(),
                    root: "/work".into(),
                    created_at: SystemTime::UNIX_EPOCH,
                    removed: false,
                    repo: true,
                    threads: vec![Thread {
                        last_session: None,
                        harness: HarnessId::new("claude"),
                        id: ThreadId(1),
                        title: None,
                        cwd: "/work".into(),
                        transcript: None,
                        status,
                        turn_started_at: None,
                        pane: Some(PaneLaunch {
                            pane: PaneId(1),
                            session: SessionId(1),
                        }),
                        branch: None,
                        created_at: SystemTime::UNIX_EPOCH,
                        last_activity_at: SystemTime::UNIX_EPOCH,
                        unseen: false,
                        model: None,
                    }],
                    kind: ProjectKind::Normal,
                }],
                cursor: Some(SidebarItem::Session(SessionId(1))),
                ..Sessions::default()
            },
            ..AppState::default()
        };
        state.sessions.sessions = sessions_for(&state.sessions.projects);
        state
    }

    #[rstest::rstest]
    fn settle_is_refused_while_an_agent_works() {
        // Given a selected session whose agent is running a turn.
        let state = on_session(ThreadStatus::Working);

        // When validating a settle.
        let result = validate_toggle_settle(&state);

        // Then validation fails with InProgress.
        assert_eq!(
            result,
            Err(ToggleSettleError::InProgress),
            "a working session can't be settled"
        );
    }

    #[rstest::rstest]
    fn unsettling_is_allowed_while_an_agent_works() {
        // Given a settled session whose agent is running a turn.
        let state = {
            let mut state = on_session(ThreadStatus::Working);
            if let Some(session) = state.sessions.sessions.first_mut() {
                session.settled_at = Some(SystemTime::UNIX_EPOCH);
            }
            state
        };

        // When validating an un-settle.
        let result = validate_toggle_settle(&state);

        // Then it's allowed.
        assert_eq!(result, Ok(()), "a settled session can always come back");
    }

    #[rstest::rstest]
    fn settle_is_refused_off_a_session() {
        // Given the cursor on nothing.
        let state = AppState::default();

        // When validating a settle.
        let result = validate_toggle_settle(&state);

        // Then validation fails with NoSession.
        assert_eq!(
            result,
            Err(ToggleSettleError::NoSession),
            "only a session settles"
        );
    }

    #[rstest::rstest]
    fn toggle_pin_allowed_on_a_session() {
        // Given a selected session.
        let state = on_session(ThreadStatus::Idle);

        // When validating a pin.
        let result = validate_toggle_pin(&state);

        // Then it's allowed.
        assert_eq!(result, Ok(()), "a session can be pinned");
    }

    #[rstest::rstest]
    fn toggle_pin_rejected_on_a_session_being_deleted() {
        // Given the selected session being deleted.
        let state = {
            let mut state = on_session(ThreadStatus::Idle);
            state.sessions.deleting.insert(SessionId(1));
            state
        };

        // When validating a pin.
        let result = validate_toggle_pin(&state);

        // Then validation fails with NoSession.
        assert_eq!(
            result,
            Err(TogglePinError::NoSession),
            "a session being deleted can't be pinned"
        );
    }

    /// [`on_session`] with the cursor on session 1's agent row for pane 1.
    fn on_agent_row() -> AppState {
        let mut state = on_session(ThreadStatus::Idle);
        state.sessions.cursor = Some(SidebarItem::Agent {
            session: SessionId(1),
            pane: PaneId(1),
        });
        state
    }

    #[rstest::rstest]
    fn toggle_pin_rejected_on_an_agent_row() {
        // Given the cursor on an agent row.
        let state = on_agent_row();

        // When validating a pin.
        let result = validate_toggle_pin(&state);

        // Then validation fails with OnAgent.
        assert_eq!(
            result,
            Err(TogglePinError::OnAgent),
            "pin is per session, not per agent"
        );
    }

    #[rstest::rstest]
    fn toggle_settle_rejected_on_an_agent_row() {
        // Given the cursor on an agent row.
        let state = on_agent_row();

        // When validating a settle.
        let result = validate_toggle_settle(&state);

        // Then validation fails with OnAgent.
        assert_eq!(
            result,
            Err(ToggleSettleError::OnAgent),
            "settle is per session, not per agent"
        );
    }

    #[rstest::rstest]
    fn delete_allowed_on_a_working_session() {
        // Given a selected session whose agent is running a turn.
        let state = on_session(ThreadStatus::Working);

        // When validating a delete.
        let result = validate_delete(&state);

        // Then it's allowed.
        assert_eq!(result, Ok(()), "a session can be deleted whatever it does");
    }

    #[rstest::rstest]
    fn delete_rejected_off_a_row() {
        // Given the cursor on nothing.
        let state = AppState::default();

        // When validating a delete.
        let result = validate_delete(&state);

        // Then validation fails with NoSelection.
        assert_eq!(
            result,
            Err(DeleteError::NoSelection),
            "nothing selected, nothing deleted"
        );
    }

    #[rstest::rstest]
    fn close_shelf_allowed_on_a_settled_session_in_the_open_shelf() {
        // Given a settled session selected in the open shelf.
        let state = {
            let mut state = on_session(ThreadStatus::Idle);
            state.sessions.shelf_open = true;
            if let Some(session) = state.sessions.sessions.first_mut() {
                session.settled_at = Some(SystemTime::UNIX_EPOCH);
            }
            state
        };

        // When validating closing the shelf.
        let result = validate_close_shelf(&state);

        // Then it's allowed.
        assert_eq!(result, Ok(()), "a settled row closes its shelf");
    }

    #[rstest::rstest]
    fn close_shelf_rejected_on_an_active_session() {
        // Given an active session selected.
        let state = on_session(ThreadStatus::Idle);

        // When validating closing the shelf.
        let result = validate_close_shelf(&state);

        // Then validation fails with NotInShelf.
        assert_eq!(
            result,
            Err(CloseShelfError::NotInShelf),
            "an active card isn't in the shelf"
        );
    }

    /// The name box for a new `kind` session holding `text`.
    fn naming(kind: FolderKind, text: &str) -> AppState {
        AppState {
            rename: Some(Rename {
                target: RenameTarget::NewFolder(kind),
                input: TextInput::new(text),
                creating: false,
            }),
            ..AppState::default()
        }
    }

    #[rstest::rstest]
    #[case("a/b", "/")]
    #[case("a\\b", "\\")]
    #[case("~x", "~")]
    #[case("a^b", "^")]
    #[case("a:b", ":")]
    #[case("a?b", "?")]
    #[case("a*b", "*")]
    #[case("a[b", "[")]
    #[case("-x", "-")]
    #[case(".x", ".")]
    #[case("a..b", "..")]
    fn new_folder_rejects_what_folders_cant_hold(#[case] text: &str, #[case] what: &str) {
        // Given the name box holding the text.
        let state = naming(FolderKind::Research, text);

        // When validating the new folder.
        let result = validate_new_folder(&state);

        // Then it names what can't be used.
        assert_eq!(
            result,
            Err(NewFolderError::Invalid(what.to_owned())),
            "{text:?} should be refused"
        );
    }

    #[rstest::rstest]
    fn new_folder_rejects_an_empty_name() {
        // Given the name box holding only spaces.
        let state = naming(FolderKind::Research, "   ");

        // When validating the new folder.
        let result = validate_new_folder(&state);

        // Then it's refused as empty.
        assert_eq!(result, Err(NewFolderError::Empty), "a blank name");
    }

    #[rstest::rstest]
    fn new_folder_while_creating_is_creating() {
        // Given the name box holding a fresh name that `⏎` already asked for.
        let mut state = naming(FolderKind::Research, "tokio cancel");
        if let Some(rename) = &mut state.rename {
            rename.creating = true;
        }

        // When validating the new folder.
        let result = validate_new_folder(&state);

        // Then it's refused until the actor answers.
        assert_eq!(result, Err(NewFolderError::Creating), "a second ⏎");
    }

    #[rstest::rstest]
    fn new_folder_allowed_for_a_fresh_slug() {
        // Given the name box holding a fresh name.
        let state = naming(FolderKind::Research, "tokio cancel");

        // When validating the new folder.
        let result = validate_new_folder(&state);

        // Then it's allowed.
        assert_eq!(result, Ok(()), "a fresh slug");
    }
}
