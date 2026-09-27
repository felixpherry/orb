//! User intents: what a key press asks orb to do.

use std::fmt;

use crate::feat::zellij::zellij_service::Tool;

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
    /// Move the keys to the selected thread's preview.
    FocusPreview,
    /// Move the keys to the sidebar.
    FocusSidebar,
    /// Hide the sidebar, giving the right side the full width, or show it
    /// again.
    ToggleSidebar,
    /// Widen the focused side: the sidebar, or the right side.
    WidenFocused,
    /// Narrow the focused side: the sidebar, or the right side.
    NarrowFocused,
    /// Attach to the selected thread's session, or start the selected draft.
    Attach,
    /// Return from the attached session to its preview.
    Detach,
    /// Leave the attached session for the sidebar, keeping its pane shown.
    LeavePane,
    /// Open the project picker to open that project's draft.
    NewSession,
    /// Open the project filter picker to filter the sidebar to one project,
    /// or to all of them.
    FilterProjects,
    /// Move the preview's cursor to the next block.
    NextBlock,
    /// Move the preview's cursor to the previous block.
    PrevBlock,
    /// Scroll the preview half a page down.
    HalfPageDown,
    /// Scroll the preview half a page up.
    HalfPageUp,
    /// Jump to the preview's first block.
    Top,
    /// Jump to the preview's bottom and follow new blocks.
    Bottom,
    /// Fold or unfold the preview's cursor block.
    ToggleFold,
    /// Copy the preview's cursor block to the clipboard.
    Yank,
    /// Pin or unpin the selected thread.
    TogglePin,
    /// Open the rename box for the selected thread, filled in with its title.
    Rename,
    /// Move the keys to the sidebar's input box to search thread titles.
    Search,
    /// Settle or un-settle the selected thread.
    ToggleSettle,
    /// Delete the selected thread and its Claude session.
    DeleteThread,
    /// Show the Settled shelf's threads.
    OpenShelf,
    /// Hide the Settled shelf's threads.
    CloseShelf,
    /// Open the directory picker to add a project.
    AddProject,
    /// Open the workspace picker for the selected draft, or the selected
    /// thread before its first prompt.
    ChangeWorkspace,
    /// Open the branch picker for the selected thread or draft.
    SwitchBranch,
    /// Open the tool in the selected thread's or draft's directory.
    OpenTool(Tool),
    /// Open the model picker for the selected draft.
    PickModel,
    /// Open the permission-mode picker for the selected draft.
    PickPermission,
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
    /// Select the picker's next item, or the sidebar search's next match.
    PickerNext,
    /// Select the picker's previous item, or the sidebar search's previous
    /// match.
    PickerPrev,
    /// Move the picker's selection half a page down.
    PickerHalfPageDown,
    /// Move the picker's selection half a page up.
    PickerHalfPageUp,
    /// Pick the selected item: open the project's draft, add the directory,
    /// or apply the workspace, branch, model or permission mode. In the
    /// rename box, save the name. In the sidebar search, end it and keep the
    /// cursor on the match.
    PickerConfirm,
    /// Browse into the directory picker's selected directory.
    PickerOpen,
    /// Close the picker without picking, or the rename box without renaming.
    /// In the sidebar search, end it and put the cursor back.
    PickerCancel,
    /// Ask to remove the project highlighted in the project filter.
    PickerRemove,
}

/// The label the which-key popup shows for the intent.
impl fmt::Display for Intent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Quit => "quit",
            Self::SelectNext => "next thread",
            Self::SelectPrev => "previous thread",
            Self::FocusPreview => "focus preview",
            Self::FocusSidebar => "focus sidebar",
            Self::ToggleSidebar | Self::LeavePane => "sidebar",
            Self::WidenFocused => "widen",
            Self::NarrowFocused => "narrow",
            Self::Attach => "attach",
            Self::Detach => "back to orb",
            Self::NewSession => "new session",
            Self::FilterProjects => "filter projects",
            Self::NextBlock => "next block",
            Self::PrevBlock => "previous block",
            Self::HalfPageDown | Self::PickerHalfPageDown | Self::SelectHalfPageDown => {
                "half page down"
            }
            Self::HalfPageUp | Self::PickerHalfPageUp | Self::SelectHalfPageUp => "half page up",
            Self::Top | Self::SelectFirst => "top",
            Self::Bottom | Self::SelectLast => "bottom",
            Self::ToggleFold => "fold",
            Self::Yank => "yank",
            Self::TogglePin => "pin",
            Self::Rename => "rename",
            Self::Search => "search",
            Self::ToggleSettle => "settle",
            Self::DeleteThread | Self::PickerBackspace => "delete",
            Self::OpenShelf => "open settled",
            Self::CloseShelf => "close settled",
            Self::AddProject => "add project",
            Self::ChangeWorkspace => "workspace",
            Self::SwitchBranch => "branch",
            Self::OpenTool(tool) => tool.label(),
            Self::PickModel => "model",
            Self::PickPermission => "permission",
            Self::PickerInput(_) => "type",
            Self::PickerDeleteWord => "delete word",
            Self::PickerCursorLeft => "cursor left",
            Self::PickerCursorRight => "cursor right",
            Self::PickerNext => "next item",
            Self::PickerPrev => "previous item",
            Self::PickerConfirm => "pick",
            Self::PickerOpen => "open directory",
            Self::PickerCancel => "cancel",
            Self::PickerRemove => "remove",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::Intent;
    use crate::feat::zellij::zellij_service::Tool;

    #[rstest::rstest]
    #[case(Tool::Shell, "shell")]
    #[case(Tool::Lazygit, "lazygit")]
    #[case(Tool::Nvim, "nvim")]
    fn open_tool_displays_the_tools_label(#[case] tool: Tool, #[case] expected: &str) {
        // Given / When / Then: which-key labels the intent by its tool.
        assert_eq!(
            Intent::OpenTool(tool).to_string(),
            expected,
            "which-key label"
        );
    }
}
