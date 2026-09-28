//! Checks whether `<C-o>`/`<C-i>` can move through the jump list.

use wherror::Error;

use crate::{AppState, Focus};

/// Why a jump back or forward can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum JumpError {
    /// No row in that direction is reachable.
    NoTarget,
    /// The keys are in the interactive `claude` asking to trust a directory.
    InTrustPane,
}

/// Allow `<C-o>` while an older reachable row is listed, outside the trust
/// pane.
///
/// # Errors
///
/// Returns [`JumpError::InTrustPane`] while the trust pane has the keys, and
/// [`JumpError::NoTarget`] when no older row is reachable.
pub fn validate_jump_back(state: &AppState) -> Result<(), JumpError> {
    outside_trust(state)?;
    state
        .jumps
        .peek_back(state.sessions.cursor, |item| state.sessions.jumpable(item))
        .map(drop)
        .ok_or(JumpError::NoTarget)
}

/// Allow `<C-i>` after a `<C-o>` while a newer reachable row is listed,
/// outside the trust pane.
///
/// # Errors
///
/// Returns [`JumpError::InTrustPane`] while the trust pane has the keys, and
/// [`JumpError::NoTarget`] when no newer row is reachable.
pub fn validate_jump_forward(state: &AppState) -> Result<(), JumpError> {
    outside_trust(state)?;
    state
        .jumps
        .peek_forward(state.sessions.cursor, |item| state.sessions.jumpable(item))
        .map(drop)
        .ok_or(JumpError::NoTarget)
}

/// Rejects a jump while the trust pane has the keys: leaving it must go
/// through `<C-\>`'s trust handling.
fn outside_trust(state: &AppState) -> Result<(), JumpError> {
    match (state.focus, &state.sessions.trust) {
        (Focus::Attached, Some(_)) => Err(JumpError::InTrustPane),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use super::{JumpError, validate_jump_back};
    use crate::feat::jumps::state::JumpList;
    use crate::feat::sessions::state::{
        Project, ProjectId, ProjectKind, Sessions, SidebarItem, Thread, ThreadId, ThreadStatus,
    };
    use crate::{AppState, Focus};

    fn thread(id: i64) -> Thread {
        Thread {
            id: ThreadId(id),
            title: None,
            cwd: "/tmp".into(),
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
        }
    }

    fn on(id: i64) -> SidebarItem {
        SidebarItem::Thread(ThreadId(id))
    }

    /// Threads 1 and 2, the cursor on thread 2, and a jump list of `jumps`.
    fn jumping(jumps: &[i64]) -> AppState {
        AppState {
            sessions: Sessions {
                projects: vec![Project {
                    id: ProjectId(1),
                    title: "work".into(),
                    root: "/work".into(),
                    created_at: SystemTime::UNIX_EPOCH,
                    removed: false,
                    draft: None,
                    threads: vec![thread(1), thread(2)],
                    groups: vec![],
                    kind: ProjectKind::Normal,
                }],
                cursor: Some(on(2)),
                ..Sessions::default()
            },
            jumps: JumpList::from_saved(jumps.iter().copied().map(on).collect()),
            ..AppState::default()
        }
    }

    #[rstest::rstest]
    fn jump_back_without_an_older_row_is_rejected() {
        // Given a jump list holding only the cursor's thread.
        let state = jumping(&[2]);

        // When validating a jump back.
        let result = validate_jump_back(&state);

        // Then there's nowhere to go.
        assert_eq!(result, Err(JumpError::NoTarget), "<C-o> needs an older row");
    }

    #[rstest::rstest]
    fn jump_back_in_the_trust_pane_is_rejected() {
        // Given the trust pane holding the keys, with an older row listed.
        let mut state = jumping(&[1, 2]);
        state.focus = Focus::Attached;
        state.sessions.trust = Some("/work".into());

        // When validating a jump back.
        let result = validate_jump_back(&state);

        // Then the trust pane keeps the keys.
        assert_eq!(
            result,
            Err(JumpError::InTrustPane),
            "<C-o> must not leave the trust pane"
        );
    }

    #[rstest::rstest]
    fn jump_back_onto_an_older_row_is_allowed() {
        // Given thread 1 listed before the cursor's thread 2.
        let state = jumping(&[1, 2]);

        // When validating a jump back.
        let result = validate_jump_back(&state);

        // Then the jump can go ahead.
        assert_eq!(result, Ok(()), "<C-o> onto thread 1 should be allowed");
    }
}
