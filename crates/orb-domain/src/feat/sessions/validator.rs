//! Checks whether the user's sidebar actions can proceed: starting the
//! selected draft or picking its model or permission mode, pinning, settling
//! or deleting the selected session, discarding the selected draft, opening
//! or closing the Settled shelf, creating a group, and opening the incognito
//! draft.

use wherror::Error;

use crate::AppState;
use crate::feat::sessions::state::{GroupKind, ProjectKind, SidebarItem, group_slug};
use crate::feat::sidebar::state::{Rename, RenameTarget};

/// Why creating a group from the name box can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum NewGroupError {
    /// The box isn't naming a new group, or the slug is empty.
    Empty,
    /// The slug uses something git branches or folder names can't: the char
    /// or sequence.
    Invalid(String),
    /// A group of that kind and slug already exists in the project: the
    /// mode-line text.
    Taken(String),
    /// `⏎` already asked for the group, and the sessions actor hasn't
    /// answered yet.
    Creating,
}

/// Characters a group's slug can't hold anywhere.
const INVALID_CHARS: [char; 8] = ['/', '\\', '~', '^', ':', '?', '*', '['];

/// The mode-line text for a group `slug` that another group of its kind in
/// its project already has.
#[must_use]
pub fn group_exists(slug: &str) -> String {
    format!("Group {slug} already exists")
}

/// The mode-line text for a `kind` group's `slug` whose branch (in `project`,
/// its title) or folder is already on disk.
#[must_use]
pub fn on_disk(kind: GroupKind, slug: &str, project: &str) -> String {
    match kind {
        GroupKind::Feature => format!("branch {slug} already exists in {project}"),
        GroupKind::Research => format!("~/.orb/research/{slug} already exists"),
        GroupKind::Learn => format!("~/.orb/learn/{slug} already exists"),
    }
}

/// Allow creating the group the name box names, once its slug is a usable
/// branch and folder name no group of its kind in its project has, settled
/// groups included.
///
/// # Errors
///
/// Returns [`NewGroupError::Empty`] when the box isn't naming a new group or
/// the slug is empty, [`NewGroupError::Invalid`] with the offending char or
/// sequence when the slug holds one of `/ \ ~ ^ : ? * [`, starts with `-` or
/// `.`, or holds `..`, [`NewGroupError::Taken`] with the mode-line text
/// when the project already has that group, and [`NewGroupError::Creating`]
/// while the box's group is being made.
pub fn validate_new_group(state: &AppState) -> Result<(), NewGroupError> {
    let Some(Rename {
        target: RenameTarget::NewGroup { kind, project },
        input,
        creating,
    }) = &state.rename
    else {
        return Err(NewGroupError::Empty);
    };
    if *creating {
        return Err(NewGroupError::Creating);
    }
    let slug = group_slug(input.text());
    let invalid = slug
        .matches(INVALID_CHARS)
        .next()
        .or_else(|| ["-", "."].into_iter().find(|lead| slug.starts_with(lead)))
        .or_else(|| slug.contains("..").then_some(".."));
    let in_project = state.sessions.projects.iter().find(|p| match kind {
        GroupKind::Feature => Some(p.id) == *project,
        GroupKind::Research => p.kind == ProjectKind::Research && !p.removed,
        GroupKind::Learn => p.kind == ProjectKind::Learn && !p.removed,
    });
    match (slug.is_empty(), invalid, in_project) {
        (true, _, _) => Err(NewGroupError::Empty),
        (false, Some(what), _) => Err(NewGroupError::Invalid(what.to_owned())),
        (false, None, Some(p)) if p.groups.iter().any(|g| g.kind == *kind && g.name == slug) => {
            Err(NewGroupError::Taken(group_exists(&slug)))
        }
        (false, None, _) => Ok(()),
    }
}

/// Why settling or un-settling can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum ToggleSettleError {
    /// The cursor isn't on a session.
    NoSession,
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
/// Returns [`ToggleSettleError::NoSession`] without a selected session, and
/// [`ToggleSettleError::InProgress`] if it isn't settled and one of its
/// agents has a turn underway.
pub fn validate_toggle_settle(state: &AppState) -> Result<(), ToggleSettleError> {
    let sessions = &state.sessions;
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
}

/// Allow pinning or unpinning the selected session.
///
/// # Errors
///
/// Returns [`TogglePinError::NoSession`] without a selected session.
pub fn validate_toggle_pin(state: &AppState) -> Result<(), TogglePinError> {
    match state.sessions.selected_session() {
        Some(_) => Ok(()),
        None => Err(TogglePinError::NoSession),
    }
}

/// Why deleting can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum DeleteError {
    /// The cursor isn't on a session, a draft or a group's draft.
    NoSelection,
    /// The cursor is on a draft or a group's draft while a session is being
    /// started, which may be starting from it.
    Starting,
}

/// Allow deleting the selected session, whatever it is doing, and
/// discarding the selected draft or group draft between session starts.
///
/// # Errors
///
/// Returns [`DeleteError::NoSelection`] without a selected session, draft or
/// group draft, and [`DeleteError::Starting`] on a draft or group draft while
/// a start is in flight.
pub fn validate_delete(state: &AppState) -> Result<(), DeleteError> {
    let sessions = &state.sessions;
    match (
        sessions.selected_session(),
        sessions.selected_draft(),
        sessions.selected_group_draft(),
    ) {
        (None, None, None) => Err(DeleteError::NoSelection),
        (None, ..) if sessions.starting => Err(DeleteError::Starting),
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

/// Why opening the incognito draft can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum NewIncognitoError {
    /// orb has no Incognito project (it adds one at start), or it was removed.
    NoProject,
}

/// Allow opening the incognito draft.
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

/// Why starting the selected group draft can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum StartGroupDraftError {
    /// The cursor isn't on a group draft.
    NoDraft,
    /// A session is already being started.
    Starting,
}

/// Allow starting the selected group draft, one start at a time.
///
/// # Errors
///
/// Returns [`StartGroupDraftError::NoDraft`] without a selected group draft,
/// and [`StartGroupDraftError::Starting`] while a start is in flight.
pub fn validate_start_group_draft(state: &AppState) -> Result<(), StartGroupDraftError> {
    match state.sessions.selected_group_draft() {
        None => Err(StartGroupDraftError::NoDraft),
        Some(_) if state.sessions.starting => Err(StartGroupDraftError::Starting),
        Some(_) => Ok(()),
    }
}

/// Why picking the selected draft's model or permission mode can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum PickSettingError {
    /// The cursor isn't on a draft or group draft.
    NoDraft,
    /// A session is being started, maybe from the draft.
    Starting,
}

/// Allow picking the selected draft's or group draft's harness, model or
/// permission mode between session starts.
///
/// # Errors
///
/// Returns [`PickSettingError::NoDraft`] without a selected draft or group
/// draft, and [`PickSettingError::Starting`] while a start is in flight.
pub fn validate_pick_setting(state: &AppState) -> Result<(), PickSettingError> {
    let sessions = &state.sessions;
    match (sessions.selected_draft(), sessions.selected_group_draft()) {
        (None, None) => Err(PickSettingError::NoDraft),
        _ if sessions.starting => Err(PickSettingError::Starting),
        _ => Ok(()),
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
        CloseShelfError, DeleteError, NewGroupError, NewIncognitoError, PickSettingError,
        StartDraftError, StartGroupDraftError, TogglePinError, ToggleSettleError,
        validate_close_shelf, validate_delete, validate_new_group, validate_new_incognito,
        validate_pick_setting, validate_start_draft, validate_start_group_draft,
        validate_toggle_pin, validate_toggle_settle,
    };
    use crate::feat::sessions::state::{
        Draft, DraftWorkspace, Group, GroupDefaults, GroupDraft, GroupId, GroupKind, PaneId,
        PaneLaunch, Project, ProjectId, ProjectKind, SessionId, Sessions, SidebarItem, Thread,
        ThreadId, ThreadStatus, sessions_for,
    };
    use crate::feat::sidebar::state::{Rename, RenameTarget};
    use crate::{AppState, TextInput};

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
                    removed: false,
                    draft: Some(Draft {
                        harness: HarnessId::new("claude"),
                        workspace: DraftWorkspace::Local,
                        branch: None,
                        model: None,
                        permission: None,
                        created_at: SystemTime::UNIX_EPOCH,
                        repo: true,
                        from: None,
                    }),
                    threads: vec![],
                    groups: vec![],
                    kind: ProjectKind::Normal,
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

    /// orb's Incognito project, `removed` or not, with no draft and nothing
    /// selected.
    fn incognito_project(removed: bool) -> AppState {
        AppState {
            sessions: Sessions {
                projects: vec![Project {
                    id: ProjectId(1),
                    title: "Incognito".into(),
                    root: "/tmp/orb-incognito".into(),
                    created_at: SystemTime::UNIX_EPOCH,
                    removed,
                    draft: None,
                    threads: vec![],
                    groups: vec![],
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

        // When validating opening the incognito draft.
        let result = validate_new_incognito(&state);

        // Then validation fails with NoProject.
        assert_eq!(
            result,
            Err(NewIncognitoError::NoProject),
            "the incognito draft needs the Incognito project"
        );
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
                    draft: None,
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
                            command: vec![],
                        }),
                        branch: None,
                        pinned_at: None,
                        settled_at: None,
                        active_since: SystemTime::UNIX_EPOCH,
                        created_at: SystemTime::UNIX_EPOCH,
                        last_activity_at: SystemTime::UNIX_EPOCH,
                        unseen: false,
                        group: None,
                        model: None,
                        permission: None,
                    }],
                    groups: vec![],
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

    /// Project `orb` (1) and orb's Research project (2), each holding a
    /// settled `GT-514-login` group of its kind, with the name box for a new
    /// `kind` group holding `text`, in `orb` for a Feature group.
    fn naming(kind: GroupKind, text: &str) -> AppState {
        let project =
            |id: i64, title: &str, project_kind: ProjectKind, group_kind: GroupKind| Project {
                id: ProjectId(id),
                title: title.into(),
                root: format!("/{title}").into(),
                created_at: SystemTime::UNIX_EPOCH,
                removed: false,
                draft: None,
                threads: vec![],
                groups: vec![Group {
                    id: GroupId(id),
                    kind: group_kind,
                    name: "GT-514-login".into(),
                    dir: None,
                    branch: None,
                    created_at: SystemTime::UNIX_EPOCH,
                    pinned_at: None,
                    settled_at: Some(SystemTime::UNIX_EPOCH),
                    active_since: SystemTime::UNIX_EPOCH,
                    draft: None,
                    defaults: GroupDefaults {
                        harness: HarnessId::new("claude"),
                        model: None,
                        permission: None,
                    },
                }],
                kind: project_kind,
            };
        AppState {
            sessions: Sessions {
                projects: vec![
                    project(1, "orb", ProjectKind::Normal, GroupKind::Feature),
                    project(2, "Research", ProjectKind::Research, GroupKind::Research),
                ],
                ..Sessions::default()
            },
            rename: Some(Rename {
                target: RenameTarget::NewGroup {
                    kind,
                    project: (kind == GroupKind::Feature).then_some(ProjectId(1)),
                },
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
    fn new_group_rejected_with_an_invalid_char(#[case] text: &str, #[case] what: &str) {
        // Given the name box holding the text.
        let state = naming(GroupKind::Research, text);

        // When validating the new group.
        let result = validate_new_group(&state);

        // Then it names what can't be used.
        assert_eq!(
            result,
            Err(NewGroupError::Invalid(what.to_owned())),
            "{text:?} should be refused"
        );
    }

    #[rstest::rstest]
    fn new_group_rejected_when_the_slug_is_empty() {
        // Given the name box holding only spaces.
        let state = naming(GroupKind::Research, "   ");

        // When validating the new group.
        let result = validate_new_group(&state);

        // Then it's refused as empty.
        assert_eq!(result, Err(NewGroupError::Empty), "a blank name");
    }

    #[rstest::rstest]
    #[case(GroupKind::Feature)]
    #[case(GroupKind::Research)]
    fn new_group_rejected_when_taken_in_the_project(#[case] kind: GroupKind) {
        // Given the name box holding a settled group's name.
        let state = naming(kind, "GT-514 login");

        // When validating the new group.
        let result = validate_new_group(&state);

        // Then it's refused because a group has the name.
        assert_eq!(
            result,
            Err(NewGroupError::Taken(
                "Group GT-514-login already exists".to_owned()
            )),
            "a taken {kind:?} slug"
        );
    }

    #[rstest::rstest]
    fn new_group_rejected_while_the_group_is_being_made() {
        // Given the name box holding a fresh name that `⏎` already asked for.
        let mut state = naming(GroupKind::Research, "tokio cancel");
        if let Some(rename) = &mut state.rename {
            rename.creating = true;
        }

        // When validating the new group.
        let result = validate_new_group(&state);

        // Then it's refused until the actor answers.
        assert_eq!(result, Err(NewGroupError::Creating), "a second ⏎");
    }

    #[rstest::rstest]
    fn new_group_allowed_for_a_fresh_slug() {
        // Given the name box holding a name no group has.
        let state = naming(GroupKind::Research, "tokio cancel");

        // When validating the new group.
        let result = validate_new_group(&state);

        // Then it's allowed.
        assert_eq!(result, Ok(()), "a fresh slug");
    }

    /// One project holding Research group 7, still a draft, with the cursor
    /// on `cursor` and a start in flight if `starting`.
    fn group_draft_at(cursor: SidebarItem, starting: bool) -> AppState {
        AppState {
            sessions: Sessions {
                projects: vec![Project {
                    id: ProjectId(1),
                    title: "Research".into(),
                    root: "/research".into(),
                    created_at: SystemTime::UNIX_EPOCH,
                    removed: false,
                    draft: None,
                    threads: vec![],
                    groups: vec![Group {
                        id: GroupId(7),
                        kind: GroupKind::Research,
                        name: "tokio-cancel".into(),
                        dir: Some("/research/tokio-cancel".into()),
                        branch: None,
                        created_at: SystemTime::UNIX_EPOCH,
                        pinned_at: None,
                        settled_at: None,
                        active_since: SystemTime::UNIX_EPOCH,
                        draft: Some(GroupDraft::default()),
                        defaults: GroupDefaults {
                            harness: HarnessId::new("claude"),
                            model: None,
                            permission: None,
                        },
                    }],
                    kind: ProjectKind::Research,
                }],
                cursor: Some(cursor),
                starting,
                ..Sessions::default()
            },
            ..AppState::default()
        }
    }

    #[rstest::rstest]
    fn start_group_draft_rejected_while_starting() {
        // Given the group draft selected while a start is in flight.
        let state = group_draft_at(SidebarItem::GroupDraft(GroupId(7)), true);

        // When validating a group draft start.
        let result = validate_start_group_draft(&state);

        // Then validation fails with Starting.
        assert_eq!(
            result,
            Err(StartGroupDraftError::Starting),
            "one start at a time"
        );
    }

    #[rstest::rstest]
    fn pick_setting_allowed_on_a_group_draft() {
        // Given the group draft selected.
        let state = group_draft_at(SidebarItem::GroupDraft(GroupId(7)), false);

        // When validating a setting pick.
        let result = validate_pick_setting(&state);

        // Then it's allowed.
        assert_eq!(result, Ok(()), "a group draft has settings to pick");
    }

    #[rstest::rstest]
    fn delete_allowed_on_a_group_draft() {
        // Given the cursor on group 7's draft.
        let state = group_draft_at(SidebarItem::GroupDraft(GroupId(7)), false);

        // When validating a delete.
        let result = validate_delete(&state);

        // Then it's allowed.
        assert_eq!(result, Ok(()), "a group's draft can be discarded");
    }

    #[rstest::rstest]
    fn delete_rejected_on_a_group_draft_while_starting() {
        // Given the cursor on group 7's draft while a start is in flight.
        let state = group_draft_at(SidebarItem::GroupDraft(GroupId(7)), true);

        // When validating a delete.
        let result = validate_delete(&state);

        // Then validation fails with Starting.
        assert_eq!(
            result,
            Err(DeleteError::Starting),
            "the draft may be starting"
        );
    }
}
