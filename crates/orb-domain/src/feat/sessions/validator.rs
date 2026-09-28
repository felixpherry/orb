//! Checks whether the user's sidebar actions can proceed: starting the
//! selected draft or picking its model or permission mode, pinning the
//! selected lone thread or group, settling or deleting the selected thread
//! or group, discarding the selected draft, opening or closing the Settled shelf or a
//! group, creating a group, and starting a group's sibling.

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
}

/// Characters a group's slug can't hold anywhere.
const INVALID_CHARS: [char; 8] = ['/', '\\', '~', '^', ':', '?', '*', '['];

/// The mode-line text for a `kind` group's `slug` that's taken in `project`
/// (its title).
#[must_use]
pub fn group_taken(kind: GroupKind, slug: &str, project: &str) -> String {
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
/// `.`, or holds `..`, and [`NewGroupError::Taken`] with the mode-line text
/// when the project already has that group.
pub fn validate_new_group(state: &AppState) -> Result<(), NewGroupError> {
    let Some(Rename {
        target: RenameTarget::NewGroup { kind, project },
        input,
    }) = &state.rename
    else {
        return Err(NewGroupError::Empty);
    };
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
            Err(NewGroupError::Taken(group_taken(*kind, &slug, &p.title)))
        }
        (false, None, _) => Ok(()),
    }
}

/// Why settling or un-settling can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum ToggleSettleError {
    /// The cursor isn't on a lone thread or a group's card.
    NoThread,
    /// Claude is running a turn or waiting on the user, in the thread or in
    /// one of the group's threads.
    InProgress,
}

/// What the mode line says when settling is refused because Claude is working.
pub const SETTLE_IN_PROGRESS: &str = "Can't settle while Claude is working";

/// Allow settling the selected lone thread or group only between turns;
/// un-settling is always allowed.
///
/// # Errors
///
/// Returns [`ToggleSettleError::NoThread`] without a selected lone thread or
/// group card, and [`ToggleSettleError::InProgress`] if the selection isn't
/// settled and it, or one of the group's threads, has a turn underway.
pub fn validate_toggle_settle(state: &AppState) -> Result<(), ToggleSettleError> {
    let sessions = &state.sessions;
    match (
        sessions.cursor,
        sessions.selected_thread(),
        sessions.selected_group(),
    ) {
        (Some(SidebarItem::Group(_)), _, Some((_, group)))
            if group.settled_at.is_none()
                && sessions
                    .group_threads(group.id)
                    .any(|thread| thread.status.in_progress()) =>
        {
            Err(ToggleSettleError::InProgress)
        }
        (_, Some(thread), None) if thread.settled_at.is_none() && thread.status.in_progress() => {
            Err(ToggleSettleError::InProgress)
        }
        (Some(SidebarItem::Group(_)), _, Some(_)) | (_, Some(_), None) => Ok(()),
        _ => Err(ToggleSettleError::NoThread),
    }
}

/// Why pinning or unpinning can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum TogglePinError {
    /// The cursor isn't on a lone thread or a group's card.
    NoThread,
}

/// Allow pinning or unpinning the selected lone thread or group.
///
/// # Errors
///
/// Returns [`TogglePinError::NoThread`] without a selected lone thread or
/// group card.
pub fn validate_toggle_pin(state: &AppState) -> Result<(), TogglePinError> {
    let sessions = &state.sessions;
    match (
        sessions.cursor,
        sessions.selected_thread(),
        sessions.selected_group(),
    ) {
        (Some(SidebarItem::Group(_)), _, Some(_)) | (_, Some(_), None) => Ok(()),
        _ => Err(TogglePinError::NoThread),
    }
}

/// Why deleting can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum DeleteError {
    /// The cursor isn't on a thread or a draft.
    NoSelection,
    /// The cursor is on a draft, or a group card with a draft, while a
    /// session is being started, which may be starting from it.
    Starting,
    /// The row is its group's last draft or thread; `d` on the card deletes
    /// the group.
    LastInGroup,
}

/// What the mode line says when `d` would leave a group empty.
pub const LAST_IN_GROUP: &str = "A group keeps at least one thread — d on the group row";

/// Allow deleting the selected thread, whatever it is doing, unless it is its
/// group's last; deleting the selected group; and discarding the selected
/// draft between session starts.
///
/// # Errors
///
/// Returns [`DeleteError::NoSelection`] without a selected thread, group or
/// draft, [`DeleteError::LastInGroup`] on a group draft or a group's last
/// thread, and [`DeleteError::Starting`] on a draft, or a card with a draft,
/// while a start is in flight.
pub fn validate_delete(state: &AppState) -> Result<(), DeleteError> {
    let sessions = &state.sessions;
    match (sessions.cursor, sessions.selected_group()) {
        (Some(SidebarItem::GroupDraft(_)), Some(_)) => Err(DeleteError::LastInGroup),
        (Some(SidebarItem::Group(_)), Some((_, group)))
            if group.draft.is_some() && sessions.starting =>
        {
            Err(DeleteError::Starting)
        }
        (Some(SidebarItem::Group(_)), Some(_)) => Ok(()),
        (Some(SidebarItem::Thread(_)), Some((_, group)))
            if sessions.group_threads(group.id).nth(1).is_none() =>
        {
            Err(DeleteError::LastInGroup)
        }
        _ => match (sessions.selected_thread(), sessions.selected_draft()) {
            (None, None) => Err(DeleteError::NoSelection),
            (None, Some(_)) if sessions.starting => Err(DeleteError::Starting),
            _ => Ok(()),
        },
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

/// Why starting a sibling in the selected group can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum NewSiblingError {
    /// The cursor isn't on a group's card or one of its threads.
    NoGroup,
    /// The group has no thread yet: its draft starts it instead.
    NoThread,
    /// A session is already being started.
    Starting,
}

/// Allow a sibling from a group's card or thread, one start at a time, once
/// the group has a thread.
///
/// # Errors
///
/// Returns [`NewSiblingError::NoGroup`] off a group's card or thread,
/// [`NewSiblingError::Starting`] while a start is in flight, and
/// [`NewSiblingError::NoThread`] for a group that has no thread yet.
pub fn validate_new_sibling(state: &AppState) -> Result<(), NewSiblingError> {
    let sessions = &state.sessions;
    let group = match sessions.cursor {
        Some(SidebarItem::Group(_) | SidebarItem::Thread(_)) => sessions.selected_group(),
        _ => None,
    };
    match group {
        None => Err(NewSiblingError::NoGroup),
        Some(_) if sessions.starting => Err(NewSiblingError::Starting),
        Some((_, group)) if sessions.group_threads(group.id).next().is_none() => {
            Err(NewSiblingError::NoThread)
        }
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

/// Allow picking the selected draft's or group draft's model or permission
/// mode between session starts.
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

/// Why opening a group can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum OpenGroupError {
    /// The cursor isn't on a group's card, draft or thread.
    NoGroup,
}

/// Allow opening the selected group.
///
/// # Errors
///
/// Returns [`OpenGroupError::NoGroup`] when the cursor isn't on a group's
/// rows.
pub fn validate_open_group(state: &AppState) -> Result<(), OpenGroupError> {
    match state.sessions.selected_group() {
        Some(_) => Ok(()),
        None => Err(OpenGroupError::NoGroup),
    }
}

/// Why closing a group can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum CloseGroupError {
    /// The cursor isn't on a group's card, draft or thread.
    NoGroup,
}

/// Allow closing the selected group.
///
/// # Errors
///
/// Returns [`CloseGroupError::NoGroup`] when the cursor isn't on a group's
/// rows.
pub fn validate_close_group(state: &AppState) -> Result<(), CloseGroupError> {
    match state.sessions.selected_group() {
        Some(_) => Ok(()),
        None => Err(CloseGroupError::NoGroup),
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use super::{
        CloseGroupError, DeleteError, NewGroupError, NewSiblingError, OpenGroupError,
        PickSettingError, StartDraftError, StartGroupDraftError, TogglePinError, ToggleSettleError,
        validate_close_group, validate_delete, validate_new_group, validate_new_sibling,
        validate_open_group, validate_pick_setting, validate_start_draft,
        validate_start_group_draft, validate_toggle_pin, validate_toggle_settle,
    };
    use crate::feat::sessions::state::{
        Draft, DraftWorkspace, Group, GroupDraft, GroupId, GroupKind, Project, ProjectId,
        ProjectKind, Sessions, SidebarItem, Thread, ThreadId, ThreadStatus,
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
                    removed: false,
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
                        group: None,
                        model: None,
                        permission: None,
                    }],
                    groups: vec![],
                    kind: ProjectKind::Normal,
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

    /// One project holding ungrouped thread 1, with the cursor on it.
    fn lone_thread() -> AppState {
        AppState {
            sessions: Sessions {
                projects: vec![Project {
                    id: ProjectId(1),
                    title: "work".into(),
                    root: "/work".into(),
                    created_at: SystemTime::UNIX_EPOCH,
                    removed: false,
                    draft: None,
                    threads: vec![Thread {
                        id: ThreadId(1),
                        title: None,
                        cwd: "/work".into(),
                        transcript: None,
                        status: ThreadStatus::Idle,
                        turn_started_at: None,
                        attach_argv: vec![],
                        branch: None,
                        pinned_at: None,
                        settled_at: None,
                        active_since: SystemTime::UNIX_EPOCH,
                        last_activity_at: SystemTime::UNIX_EPOCH,
                        unseen: false,
                        group: None,
                        model: None,
                        permission: None,
                    }],
                    groups: vec![],
                    kind: ProjectKind::Normal,
                }],
                cursor: Some(SidebarItem::Thread(ThreadId(1))),
                ..Sessions::default()
            },
            ..AppState::default()
        }
    }

    #[rstest::rstest]
    fn open_group_rejected_off_a_group() {
        // Given the cursor on a thread in no group.
        let state = lone_thread();

        // When validating a group open.
        let result = validate_open_group(&state);

        // Then validation fails with NoGroup.
        assert_eq!(
            result,
            Err(OpenGroupError::NoGroup),
            "only a group's rows can open a group"
        );
    }

    #[rstest::rstest]
    fn close_group_rejected_off_a_group() {
        // Given the cursor on a thread in no group.
        let state = lone_thread();

        // When validating a group close.
        let result = validate_close_group(&state);

        // Then validation fails with NoGroup.
        assert_eq!(
            result,
            Err(CloseGroupError::NoGroup),
            "only a group's rows can close a group"
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
    #[case(GroupKind::Feature, "branch GT-514-login already exists in orb")]
    #[case(GroupKind::Research, "~/.orb/research/GT-514-login already exists")]
    fn new_group_rejected_when_taken_in_the_project(
        #[case] kind: GroupKind,
        #[case] expected: &str,
    ) {
        // Given the name box holding a settled group's name.
        let state = naming(kind, "GT-514 login");

        // When validating the new group.
        let result = validate_new_group(&state);

        // Then it's refused with the taken text.
        assert_eq!(
            result,
            Err(NewGroupError::Taken(expected.to_owned())),
            "a taken {kind:?} slug"
        );
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
                        draft: Some(GroupDraft {
                            model: None,
                            permission: None,
                        }),
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
    fn start_group_draft_rejected_off_a_group_draft() {
        // Given the cursor on the group's card.
        let state = group_draft_at(SidebarItem::Group(GroupId(7)), false);

        // When validating a group draft start.
        let result = validate_start_group_draft(&state);

        // Then validation fails with NoDraft.
        assert_eq!(
            result,
            Err(StartGroupDraftError::NoDraft),
            "only a group draft row starts a group"
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

    /// One project holding Feature group 7, whose threads have `statuses`,
    /// newest first (thread ids count down to 1), with the cursor on `cursor`
    /// and a start in flight if `starting`.
    fn grouped_at(cursor: SidebarItem, statuses: &[ThreadStatus], starting: bool) -> AppState {
        let threads = statuses
            .iter()
            .zip((1..=statuses.len()).rev())
            .map(|(status, id)| Thread {
                id: ThreadId(i64::try_from(id).unwrap_or_default()),
                title: None,
                cwd: "/work/GT-514-login".into(),
                transcript: None,
                status: *status,
                turn_started_at: None,
                attach_argv: vec![],
                branch: None,
                pinned_at: None,
                settled_at: None,
                active_since: SystemTime::UNIX_EPOCH,
                last_activity_at: SystemTime::UNIX_EPOCH,
                unseen: false,
                group: Some(GroupId(7)),
                model: None,
                permission: None,
            })
            .collect();
        AppState {
            sessions: Sessions {
                projects: vec![Project {
                    id: ProjectId(1),
                    title: "work".into(),
                    root: "/work".into(),
                    created_at: SystemTime::UNIX_EPOCH,
                    removed: false,
                    draft: None,
                    threads,
                    groups: vec![Group {
                        id: GroupId(7),
                        kind: GroupKind::Feature,
                        name: "GT-514-login".into(),
                        dir: Some("/work/GT-514-login".into()),
                        branch: Some("GT-514-login".into()),
                        created_at: SystemTime::UNIX_EPOCH,
                        pinned_at: None,
                        settled_at: None,
                        active_since: SystemTime::UNIX_EPOCH,
                        draft: None,
                    }],
                    kind: ProjectKind::Normal,
                }],
                cursor: Some(cursor),
                starting,
                ..Sessions::default()
            },
            ..AppState::default()
        }
    }

    #[rstest::rstest]
    fn new_sibling_rejected_on_a_lone_thread() {
        // Given the cursor on a thread in no group.
        let state = lone_thread();

        // When validating a sibling start.
        let result = validate_new_sibling(&state);

        // Then validation fails with NoGroup.
        assert_eq!(
            result,
            Err(NewSiblingError::NoGroup),
            "a lone thread has no group"
        );
    }

    #[rstest::rstest]
    fn new_sibling_rejected_on_a_group_draft() {
        // Given the cursor on a group's draft.
        let state = group_draft_at(SidebarItem::GroupDraft(GroupId(7)), false);

        // When validating a sibling start.
        let result = validate_new_sibling(&state);

        // Then validation fails with NoGroup.
        assert_eq!(
            result,
            Err(NewSiblingError::NoGroup),
            "a group draft starts its group instead"
        );
    }

    #[rstest::rstest]
    fn new_sibling_rejected_while_starting() {
        // Given the cursor on a group's card while a start is in flight.
        let state = grouped_at(SidebarItem::Group(GroupId(7)), &[ThreadStatus::Idle], true);

        // When validating a sibling start.
        let result = validate_new_sibling(&state);

        // Then validation fails with Starting.
        assert_eq!(
            result,
            Err(NewSiblingError::Starting),
            "one start at a time"
        );
    }

    #[rstest::rstest]
    fn new_sibling_rejected_without_a_thread() {
        // Given the cursor on the card of a group that's still a draft.
        let state = group_draft_at(SidebarItem::Group(GroupId(7)), false);

        // When validating a sibling start.
        let result = validate_new_sibling(&state);

        // Then validation fails with NoThread.
        assert_eq!(
            result,
            Err(NewSiblingError::NoThread),
            "a draft-only group starts from its draft"
        );
    }

    #[rstest::rstest]
    fn new_sibling_allowed_on_a_card() {
        // Given the cursor on the card of a group with a thread.
        let state = grouped_at(SidebarItem::Group(GroupId(7)), &[ThreadStatus::Idle], false);

        // When validating a sibling start.
        let result = validate_new_sibling(&state);

        // Then it's allowed.
        assert_eq!(result, Ok(()), "a group with a thread takes a sibling");
    }

    #[rstest::rstest]
    fn toggle_pin_rejected_on_a_grouped_thread() {
        // Given the cursor on a thread of group 7.
        let state = grouped_at(
            SidebarItem::Thread(ThreadId(1)),
            &[ThreadStatus::Idle],
            false,
        );

        // When validating a pin.
        let result = validate_toggle_pin(&state);

        // Then validation fails with NoThread.
        assert_eq!(
            result,
            Err(TogglePinError::NoThread),
            "a grouped thread is pinned with its group"
        );
    }

    #[rstest::rstest]
    fn toggle_pin_allowed_on_a_group_card() {
        // Given the cursor on group 7's card.
        let state = grouped_at(SidebarItem::Group(GroupId(7)), &[ThreadStatus::Idle], false);

        // When validating a pin.
        let result = validate_toggle_pin(&state);

        // Then it's allowed.
        assert_eq!(result, Ok(()), "a group card can be pinned");
    }

    #[rstest::rstest]
    fn toggle_settle_rejected_on_a_grouped_thread() {
        // Given the cursor on a thread of group 7.
        let state = grouped_at(
            SidebarItem::Thread(ThreadId(1)),
            &[ThreadStatus::Idle],
            false,
        );

        // When validating a settle.
        let result = validate_toggle_settle(&state);

        // Then validation fails with NoThread.
        assert_eq!(
            result,
            Err(ToggleSettleError::NoThread),
            "a grouped thread is settled with its group"
        );
    }

    #[rstest::rstest]
    fn toggle_settle_rejected_on_a_card_with_a_working_thread() {
        // Given the cursor on group 7's card while one of its threads works.
        let state = grouped_at(
            SidebarItem::Group(GroupId(7)),
            &[ThreadStatus::Working, ThreadStatus::Idle],
            false,
        );

        // When validating a settle.
        let result = validate_toggle_settle(&state);

        // Then validation fails with InProgress.
        assert_eq!(
            result,
            Err(ToggleSettleError::InProgress),
            "a group with a turn underway can't settle"
        );
    }

    #[rstest::rstest]
    fn toggle_settle_allowed_on_a_settled_card_with_a_working_thread() {
        // Given settled group 7's card selected while one of its threads works.
        let mut state = grouped_at(
            SidebarItem::Group(GroupId(7)),
            &[ThreadStatus::Working],
            false,
        );
        if let Some(group) = state
            .sessions
            .projects
            .iter_mut()
            .flat_map(|project| &mut project.groups)
            .next()
        {
            group.settled_at = Some(SystemTime::UNIX_EPOCH);
        }

        // When validating a settle.
        let result = validate_toggle_settle(&state);

        // Then it's allowed: un-settling always is.
        assert_eq!(result, Ok(()), "a settled group can always un-settle");
    }

    #[rstest::rstest]
    fn delete_rejected_on_a_groups_last_thread() {
        // Given the cursor on group 7's only thread.
        let state = grouped_at(
            SidebarItem::Thread(ThreadId(1)),
            &[ThreadStatus::Idle],
            false,
        );

        // When validating a delete.
        let result = validate_delete(&state);

        // Then validation fails with LastInGroup.
        assert_eq!(
            result,
            Err(DeleteError::LastInGroup),
            "a group keeps its last thread"
        );
    }

    #[rstest::rstest]
    fn delete_rejected_on_a_group_draft() {
        // Given the cursor on group 7's draft.
        let state = group_draft_at(SidebarItem::GroupDraft(GroupId(7)), false);

        // When validating a delete.
        let result = validate_delete(&state);

        // Then validation fails with LastInGroup.
        assert_eq!(
            result,
            Err(DeleteError::LastInGroup),
            "a group keeps its draft"
        );
    }

    #[rstest::rstest]
    fn delete_allowed_on_a_grouped_thread_with_a_sibling() {
        // Given the cursor on thread 1 of group 7, beside thread 2.
        let state = grouped_at(
            SidebarItem::Thread(ThreadId(1)),
            &[ThreadStatus::Idle, ThreadStatus::Idle],
            false,
        );

        // When validating a delete.
        let result = validate_delete(&state);

        // Then it's allowed.
        assert_eq!(result, Ok(()), "a sibling keeps the group");
    }

    #[rstest::rstest]
    fn delete_rejected_on_a_card_whose_draft_is_starting() {
        // Given the card of draft-only group 7 selected while a start is in
        // flight.
        let state = group_draft_at(SidebarItem::Group(GroupId(7)), true);

        // When validating a delete.
        let result = validate_delete(&state);

        // Then validation fails with Starting.
        assert_eq!(
            result,
            Err(DeleteError::Starting),
            "the draft may be starting"
        );
    }

    #[rstest::rstest]
    fn delete_allowed_on_a_group_card() {
        // Given the cursor on group 7's card.
        let state = grouped_at(SidebarItem::Group(GroupId(7)), &[ThreadStatus::Idle], false);

        // When validating a delete.
        let result = validate_delete(&state);

        // Then it's allowed.
        assert_eq!(result, Ok(()), "a group card deletes the group");
    }
}
