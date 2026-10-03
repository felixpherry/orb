//! User intents: what a key press asks orb to do.

use std::fmt;

use crate::feat::sessions::state::{GroupKind, SidebarItem};
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
    /// Move the keys to the right-hand area.
    FocusRight,
    /// Move the keys to the sidebar.
    FocusSidebar,
    /// Highlight the dashboard's next menu item.
    DashboardNext,
    /// Highlight the dashboard's previous menu item.
    DashboardPrev,
    /// Run the dashboard's highlighted menu item.
    DashboardRun,
    /// Select this sidebar row (a click). During a search, end it on that
    /// row as `⏎` does.
    SelectRow(SidebarItem),
    /// Move the sidebar's cursor one row down, stopping on the last row (the
    /// wheel).
    SelectWheelNext,
    /// Move the sidebar's cursor one row up, stopping on the first row (the
    /// wheel).
    SelectWheelPrev,
    /// Highlight the dashboard's menu item at this index without running it
    /// (a click).
    DashboardHighlight(usize),
    /// Hide the sidebar, giving the right side the full width, or show it
    /// again.
    ToggleSidebar,
    /// Widen the focused side: the sidebar, or the right side.
    WidenFocused,
    /// Narrow the focused side: the sidebar, or the right side.
    NarrowFocused,
    /// Attach to the selected thread's session, or start the selected draft,
    /// or open or close the selected group.
    Attach,
    /// Return from the attached session to the dashboard.
    Detach,
    /// Leave the attached session for the sidebar, keeping its pane shown.
    LeavePane,
    /// Detach the selected thread from its pane, keeping the keys where they are.
    DetachSelected,
    /// Move back to the previous row in the jump list.
    JumpBack,
    /// Move forward to the next row in the jump list, after a jump back.
    JumpForward,
    /// Open the project picker to open that project's draft.
    NewSession,
    /// Open orb's Incognito project's draft, creating it when it has none,
    /// like picking a project in the project picker; `⏎` then starts it.
    NewIncognito,
    /// Open the session picker over the threads inside the project filter,
    /// newest chat first, to jump into one.
    OpenSessionPicker,
    /// Open the project filter picker to filter the sidebar to one project,
    /// or to all of them.
    FilterProjects,
    /// Pin or unpin the selected lone thread or group.
    TogglePin,
    /// Open the rename box for the selected thread, filled in with its title.
    Rename,
    /// Move the keys to the sidebar's input box to search thread titles.
    Search,
    /// Un-settle the selected lone thread or group, or ask to settle it.
    ToggleSettle,
    /// Ask to delete the selected thread or group and its Claude sessions, or
    /// to discard the selected draft.
    DeleteThread,
    /// Show the Settled shelf's threads.
    OpenShelf,
    /// Hide the Settled shelf's threads.
    CloseShelf,
    /// Show the selected group's children.
    OpenGroup,
    /// Hide the selected group's children and select its card; on a settled
    /// group already closed, close the Settled shelf.
    CloseGroup,
    /// Start a new group of `kind`: pick its project (Feature), then name it.
    NewGroup(GroupKind),
    /// Start a sibling at the top of the selected group, in its directory,
    /// with the selected thread's model and permission (the newest thread's
    /// on the card).
    NewSibling,
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
            Self::SelectNext | Self::SelectWheelNext => "next thread",
            Self::SelectPrev | Self::SelectWheelPrev => "previous thread",
            Self::SelectRow(_) => "select",
            Self::DashboardHighlight(_) => "highlight item",
            Self::FocusRight => "focus right",
            Self::FocusSidebar => "focus sidebar",
            Self::DashboardNext | Self::PickerNext => "next item",
            Self::DashboardPrev | Self::PickerPrev => "previous item",
            Self::DashboardRun => "run item",
            Self::ToggleSidebar | Self::LeavePane => "sidebar",
            Self::WidenFocused => "widen",
            Self::NarrowFocused => "narrow",
            Self::Attach => "attach",
            Self::Detach => "back to orb",
            Self::DetachSelected => "detach",
            Self::JumpBack => "jump back",
            Self::JumpForward => "jump forward",
            Self::NewSession => "new session",
            Self::NewIncognito => "incognito",
            Self::OpenSessionPicker => "sessions",
            Self::FilterProjects => "filter projects",
            Self::PickerHalfPageDown | Self::SelectHalfPageDown => "half page down",
            Self::PickerHalfPageUp | Self::SelectHalfPageUp => "half page up",
            Self::SelectFirst => "top",
            Self::SelectLast => "bottom",
            Self::TogglePin => "pin",
            Self::Rename => "rename",
            Self::Search => "search",
            Self::ToggleSettle => "settle",
            Self::DeleteThread | Self::PickerBackspace => "delete",
            Self::OpenShelf => "open settled",
            Self::CloseShelf => "close settled",
            Self::OpenGroup => "open group",
            Self::CloseGroup => "close group",
            Self::NewGroup(GroupKind::Feature) => "feature group",
            Self::NewGroup(GroupKind::Research) => "research group",
            Self::NewGroup(GroupKind::Learn) => "learn group",
            Self::NewSibling => "new sibling",
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
    use crate::feat::sessions::state::GroupKind;
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

    #[rstest::rstest]
    #[case(Intent::OpenSessionPicker, "sessions")]
    #[case(Intent::PickerToggleSettled, "toggle settled")]
    fn session_picker_intents_display_their_labels(#[case] intent: Intent, #[case] expected: &str) {
        // Given / When / Then: which-key labels the session picker's intents.
        assert_eq!(intent.to_string(), expected, "which-key label");
    }

    #[rstest::rstest]
    fn detach_selected_displays_as_detach() {
        // Given / When / Then: which-key labels DetachSelected "detach".
        assert_eq!(
            Intent::DetachSelected.to_string(),
            "detach",
            "which-key label"
        );
    }

    #[rstest::rstest]
    #[case(GroupKind::Feature, "feature group")]
    #[case(GroupKind::Research, "research group")]
    #[case(GroupKind::Learn, "learn group")]
    fn new_group_displays_its_kind(#[case] kind: GroupKind, #[case] expected: &str) {
        // Given / When / Then: which-key labels NewGroup by its kind.
        assert_eq!(
            Intent::NewGroup(kind).to_string(),
            expected,
            "which-key label"
        );
    }

    #[rstest::rstest]
    fn new_sibling_displays_as_new_sibling() {
        // Given / When / Then: which-key labels NewSibling "new sibling".
        assert_eq!(
            Intent::NewSibling.to_string(),
            "new sibling",
            "which-key label"
        );
    }
}
