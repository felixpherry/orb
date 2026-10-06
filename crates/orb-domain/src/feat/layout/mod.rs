//! Tabs and splits: each attached session's tabs, each tab's panes laid out
//! as a tree of right and down splits, which pane has the keys, and whether
//! a tab is zoomed onto it.

pub mod state;
pub mod tree;
pub mod validator;
