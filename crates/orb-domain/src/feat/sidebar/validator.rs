//! Checks whether the user can resize the sidebar or move the keys to it:
//! neither while it's hidden.

use wherror::Error;

use crate::AppState;

/// Why resizing can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum ResizeError {
    /// The sidebar is hidden.
    Hidden,
}

/// Allow resizing only while the sidebar is shown.
///
/// # Errors
///
/// Returns [`ResizeError::Hidden`] while the sidebar is hidden.
pub fn validate_resize(state: &AppState) -> Result<(), ResizeError> {
    if state.sidebar.hidden {
        Err(ResizeError::Hidden)
    } else {
        Ok(())
    }
}

/// Why focusing the sidebar can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum FocusSidebarError {
    /// The sidebar is hidden.
    Hidden,
}

/// Allow focusing the sidebar only while it's shown; `␣e` shows it.
///
/// # Errors
///
/// Returns [`FocusSidebarError::Hidden`] while the sidebar is hidden.
pub fn validate_focus_sidebar(state: &AppState) -> Result<(), FocusSidebarError> {
    if state.sidebar.hidden {
        Err(FocusSidebarError::Hidden)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{FocusSidebarError, ResizeError, validate_focus_sidebar, validate_resize};
    use crate::AppState;
    use crate::feat::sidebar::state::SidebarView;

    fn hidden() -> AppState {
        AppState {
            sidebar: SidebarView {
                hidden: true,
                ..SidebarView::default()
            },
            ..AppState::default()
        }
    }

    #[rstest::rstest]
    fn resize_rejected_while_hidden() {
        // Given a hidden sidebar.
        let state = hidden();

        // When validating a resize.
        let result = validate_resize(&state);

        // Then validation fails with Hidden.
        assert_eq!(
            result,
            Err(ResizeError::Hidden),
            "a hidden sidebar can't be resized"
        );
    }

    #[rstest::rstest]
    fn focus_sidebar_rejected_while_hidden() {
        // Given a hidden sidebar.
        let state = hidden();

        // When validating focusing it.
        let result = validate_focus_sidebar(&state);

        // Then validation fails with Hidden.
        assert_eq!(
            result,
            Err(FocusSidebarError::Hidden),
            "a hidden sidebar can't take the keys"
        );
    }
}
