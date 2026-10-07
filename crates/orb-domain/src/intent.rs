//! User intents: what a key press asks orb to do.

use std::fmt;

use crate::feat::layout::tree::{NavDirection, Split};
use crate::feat::sessions::state::{FolderKind, PaneId, SidebarItem};

/// A user action produced by the keymap and applied by the
/// [`IntentHandler`](crate::IntentHandler).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Intent {
    /// Quit orb; sessions keep running.
    Quit,
    /// Move the sidebar's cursor to the row below.
    SelectNext,
    /// Move the sidebar's cursor to the row above.
    SelectPrev,
    /// Move the sidebar's cursor to the first row.
    SelectFirst,
    /// Move the sidebar's cursor to the last row.
    SelectLast,
    /// Move the sidebar's cursor down half its visible height.
    SelectHalfPageDown,
    /// Move the sidebar's cursor up half its visible height.
    SelectHalfPageUp,
    /// Select this sidebar row (a click). During a search, end it on that
    /// row as `⏎` does.
    SelectRow(SidebarItem),
    /// Move the sidebar's cursor one row down, stopping on the last row (the
    /// wheel).
    SelectWheelNext,
    /// Move the sidebar's cursor one row up, stopping on the first row (the
    /// wheel).
    SelectWheelPrev,
    /// Hide the sidebar, giving the right side the full width, or show it
    /// again.
    ToggleSidebar,
    /// Move the focus to the pane that way; from the leftmost pane to the
    /// sidebar, from the rightmost to the next tab, and from the sidebar
    /// (`Right`) into the shown layout.
    MoveFocus(NavDirection),
    /// Split the focused pane, the new shell taking the focus.
    SplitPane(Split),
    /// Close the focused pane; on the thread's own pane, detach the thread.
    ClosePane,
    /// Show only the focused pane over its tab, or every pane again.
    ToggleZoom,
    /// Grow the focused pane, or widen the sidebar while it has the keys.
    GrowFocused,
    /// Shrink the focused pane, or narrow the sidebar while it has the keys.
    ShrinkFocused,
    /// Open a tab with one shell, shown.
    NewTab,
    /// Close the shown tab; one holding a pane a thread runs in detaches the thread.
    CloseTab,
    /// Open the rename box for the shown tab.
    RenameTab,
    /// Open the rename box for the focused pane.
    RenamePane,
    /// Show tab N, counting from 1.
    GoToTab(usize),
    /// Show the next tab, wrapping to the first.
    NextTab,
    /// Show the previous tab, wrapping to the last.
    PreviousTab,
    /// Swap the shown tab with the one before it.
    MoveTabLeft,
    /// Swap the shown tab with the one after it.
    MoveTabRight,
    /// Focus this pane of the shown tab and move the keys there (a click).
    FocusPane(PaneId),
    /// Attach to the selected session (un-settling a settled one), or open or
    /// close the Settled shelf on its header.
    Attach,
    /// Move the keys from a pane to the sidebar, keeping the panes shown.
    LeavePane,
    /// Send Ctrl g to the focused pane's program (`<C-g> <C-g>`, since
    /// `<C-g>` alone is orb's leader).
    SendCtrlG,
    /// Move back to the previous row in the jump list.
    JumpBack,
    /// Move forward to the next row in the jump list, after a jump back.
    JumpForward,
    /// Open the project picker to make a session of that project.
    NewSession,
    /// Make a session in orb's Incognito folder, with one shell.
    NewIncognito,
    /// Open the session picker over the threads inside the project filter,
    /// newest chat first, to jump into one.
    OpenSessionPicker,
    /// Open the worktree picker over every directory under
    /// `~/.orb/worktrees/<repo>/`, most recently used first.
    OpenWorktreePicker,
    /// Open the search picker, which finds typed text in the prompts and
    /// replies of every thread's transcripts, newest message first.
    OpenSearch,
    /// Open the project filter picker to filter the sidebar to one project,
    /// or to all of them.
    FilterProjects,
    /// Pin or unpin the selected session.
    TogglePin,
    /// Open the rename box for the selected session, filled in with its
    /// title.
    Rename,
    /// Move the keys to the sidebar's input box to search thread titles.
    Search,
    /// Un-settle the selected session, or ask to settle it.
    ToggleSettle,
    /// Ask to delete the selected session.
    Delete,
    /// Show the Settled shelf's sessions.
    OpenShelf,
    /// Hide the Settled shelf's sessions.
    CloseShelf,
    /// Name a new session in orb's own `kind` folder.
    NewFolder(FolderKind),
    /// Open the directory picker to add a project.
    AddProject,
    /// Open the workspace picker for the selected thread before its first
    /// prompt.
    ChangeWorkspace,
    /// Open the branch picker for the selected thread.
    SwitchBranch,
    /// Type a character into the picker's filter, the rename box, or the
    /// sidebar search.
    PickerInput(char),
    /// Delete the grapheme before the cursor of the picker, the rename box, or
    /// the sidebar search.
    PickerBackspace,
    /// Delete the word before the cursor of the picker, the rename box, or
    /// the sidebar search.
    PickerDeleteWord,
    /// Move the cursor of the picker, the rename box, or the sidebar search
    /// one grapheme left.
    PickerCursorLeft,
    /// Move the cursor of the picker, the rename box, or the sidebar search
    /// one grapheme right.
    PickerCursorRight,
    /// Move the cursor of the picker, the rename box, or the sidebar search
    /// to this grapheme (a click), or to the end when the text is shorter.
    PickerCursorTo(usize),
    /// Select the picker's next item, or the sidebar search's next match.
    PickerNext,
    /// Select the picker's previous item, or the sidebar search's previous
    /// match.
    PickerPrev,
    /// Move the picker's selection half a page down.
    PickerHalfPageDown,
    /// Move the picker's selection half a page up.
    PickerHalfPageUp,
    /// Select the picker's shown row at this index (a click). A heading, a
    /// disabled row or an index past the end changes nothing.
    PickerSelectRow(usize),
    /// Select the picker's next item, stopping on the last (the wheel).
    PickerWheelNext,
    /// Select the picker's previous item, stopping on the first (the wheel).
    PickerWheelPrev,
    /// Pick the selected item: go on with the project's new session, add the
    /// directory, or apply the workspace, base, branch, model or permission
    /// mode. In the
    /// rename box, save the name. In the sidebar search, end it and keep the
    /// cursor on the match, or with no match, cancel it.
    PickerConfirm,
    /// Browse into the directory picker's selected directory.
    PickerOpen,
    /// Close the picker without picking, or the rename box without renaming.
    /// In the sidebar search, end it and put the cursor back.
    PickerCancel,
    /// Ask to remove the project highlighted in the project filter.
    PickerRemove,
    /// Show or hide settled threads in the session picker, keeping the typed
    /// text.
    PickerToggleSettled,
}

/// The label the which-key popup shows for the intent.
impl fmt::Display for Intent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Quit => "quit",
            Self::SelectNext | Self::SelectWheelNext => "next session",
            Self::SelectPrev | Self::SelectWheelPrev => "previous session",
            Self::SelectRow(_) | Self::PickerSelectRow(_) => "select",
            Self::MoveFocus(NavDirection::Right) => "focus right",
            Self::PickerNext | Self::PickerWheelNext => "next item",
            Self::PickerPrev | Self::PickerWheelPrev => "previous item",
            Self::ToggleSidebar | Self::LeavePane => "sidebar",
            Self::MoveFocus(NavDirection::Left) => "focus left",
            Self::MoveFocus(NavDirection::Up) => "focus up",
            Self::MoveFocus(NavDirection::Down) => "focus down",
            Self::SplitPane(Split::Right) => "split right",
            Self::SplitPane(Split::Down) => "split down",
            Self::ClosePane => "close pane",
            Self::ToggleZoom => "zoom",
            Self::GrowFocused => "grow",
            Self::ShrinkFocused => "shrink",
            Self::NewTab => "new tab",
            Self::CloseTab => "close tab",
            Self::RenameTab => "rename tab",
            Self::RenamePane => "rename pane",
            Self::GoToTab(_) => "tab",
            Self::NextTab => "next tab",
            Self::PreviousTab => "previous tab",
            Self::MoveTabLeft => "move tab left",
            Self::MoveTabRight => "move tab right",
            Self::FocusPane(_) => "focus pane",
            Self::SendCtrlG => "send ctrl g",
            Self::Attach => "attach",
            Self::JumpBack => "jump back",
            Self::JumpForward => "jump forward",
            Self::NewSession => "new session",
            Self::NewIncognito => "incognito",
            Self::OpenSessionPicker => "sessions",
            Self::OpenWorktreePicker => "worktrees",
            Self::OpenSearch => "grep",
            Self::FilterProjects => "filter projects",
            Self::PickerHalfPageDown | Self::SelectHalfPageDown => "half page down",
            Self::PickerHalfPageUp | Self::SelectHalfPageUp => "half page up",
            Self::SelectFirst => "top",
            Self::SelectLast => "bottom",
            Self::TogglePin => "pin",
            Self::Rename => "rename",
            Self::Search => "search",
            Self::ToggleSettle => "settle",
            Self::Delete | Self::PickerBackspace => "delete",
            Self::OpenShelf => "open settled",
            Self::CloseShelf => "close settled",
            Self::NewFolder(FolderKind::Research) => "research session",
            Self::NewFolder(FolderKind::Learn) => "learn session",
            Self::AddProject => "add project",
            Self::ChangeWorkspace => "workspace",
            Self::SwitchBranch => "branch",
            Self::PickerInput(_) => "type",
            Self::PickerDeleteWord => "delete word",
            Self::PickerCursorLeft => "cursor left",
            Self::PickerCursorRight => "cursor right",
            Self::PickerCursorTo(_) => "move cursor",
            Self::PickerConfirm => "pick",
            Self::PickerOpen => "open directory",
            Self::PickerCancel => "cancel",
            Self::PickerRemove => "remove",
            Self::PickerToggleSettled => "toggle settled",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::Intent;
    use crate::feat::layout::tree::{NavDirection, Split};
    use crate::feat::sessions::state::{FolderKind, PaneId};

    #[rstest::rstest]
    #[case(Intent::MoveFocus(NavDirection::Left), "focus left")]
    #[case(Intent::MoveFocus(NavDirection::Right), "focus right")]
    #[case(Intent::MoveFocus(NavDirection::Up), "focus up")]
    #[case(Intent::MoveFocus(NavDirection::Down), "focus down")]
    #[case(Intent::SplitPane(Split::Right), "split right")]
    #[case(Intent::SplitPane(Split::Down), "split down")]
    #[case(Intent::ClosePane, "close pane")]
    #[case(Intent::ToggleZoom, "zoom")]
    #[case(Intent::GrowFocused, "grow")]
    #[case(Intent::ShrinkFocused, "shrink")]
    #[case(Intent::NewTab, "new tab")]
    #[case(Intent::CloseTab, "close tab")]
    #[case(Intent::RenameTab, "rename tab")]
    #[case(Intent::RenamePane, "rename pane")]
    #[case(Intent::GoToTab(3), "tab")]
    #[case(Intent::NextTab, "next tab")]
    #[case(Intent::PreviousTab, "previous tab")]
    #[case(Intent::MoveTabLeft, "move tab left")]
    #[case(Intent::MoveTabRight, "move tab right")]
    #[case(Intent::FocusPane(PaneId(-1)), "focus pane")]
    #[case(Intent::SendCtrlG, "send ctrl g")]
    fn layout_intents_display_their_labels(#[case] intent: Intent, #[case] expected: &str) {
        // Given / When / Then: which-key labels the layout intent.
        assert_eq!(intent.to_string(), expected, "which-key label");
    }

    #[rstest::rstest]
    #[case(Intent::OpenSessionPicker, "sessions")]
    #[case(Intent::PickerToggleSettled, "toggle settled")]
    fn session_picker_intents_display_their_labels(#[case] intent: Intent, #[case] expected: &str) {
        // Given / When / Then: which-key labels the session picker's intents.
        assert_eq!(intent.to_string(), expected, "which-key label");
    }

    #[rstest::rstest]
    fn open_worktree_picker_displays_as_worktrees() {
        // Given / When / Then: which-key labels OpenWorktreePicker "worktrees".
        assert_eq!(
            Intent::OpenWorktreePicker.to_string(),
            "worktrees",
            "which-key label"
        );
    }

    #[rstest::rstest]
    fn open_search_displays_as_grep() {
        // Given / When / Then: which-key labels OpenSearch "grep".
        assert_eq!(Intent::OpenSearch.to_string(), "grep", "which-key label");
    }

    #[rstest::rstest]
    #[case(FolderKind::Research, "research session")]
    #[case(FolderKind::Learn, "learn session")]
    fn new_folder_displays_its_kind(#[case] kind: FolderKind, #[case] expected: &str) {
        // Given / When / Then: which-key labels NewFolder by its kind.
        assert_eq!(
            Intent::NewFolder(kind).to_string(),
            expected,
            "which-key label"
        );
    }
}
