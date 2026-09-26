//! The sidebar's size and visibility, and the last frame's row layout.

/// The sidebar's width in columns until the user resizes it.
pub const DEFAULT_WIDTH: u16 = 32;

/// How the sidebar is laid out. The width is written by the intent handler
/// and, on restore, the sessions actor; `hidden` by the intent handler;
/// `layout` by the frontend after each draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarView {
    /// Columns the sidebar takes while shown.
    pub width: u16,
    /// The sidebar is out of the way and the right-hand area takes the full
    /// width.
    pub hidden: bool,
    /// The last frame's list height and row heights.
    pub layout: SidebarLayout,
}

impl Default for SidebarView {
    fn default() -> Self {
        Self {
            width: DEFAULT_WIDTH,
            hidden: false,
            layout: SidebarLayout::default(),
        }
    }
}

/// How the last frame laid the sidebar's list out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SidebarLayout {
    /// Lines the list is drawn in.
    pub rows: u16,
    /// One per sidebar row, in the sidebar's order.
    pub heights: Vec<u16>,
}
