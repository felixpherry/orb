//! Terminal pane for orb: runs a program in a PTY and shows its screen inside
//! orb.
//!
//! A [`Pane`] owns one child process. Its output is emulated in the
//! background, including the replies to the queries the child sends at
//! startup, so the owner only has to draw it. The user's keys, pastes, mouse
//! events (in pixels for a child that asks), and focus changes are encoded for
//! whatever terminal modes the child has switched on and written straight to
//! it. The owner is told when the screen changed, when the child copied to
//! the clipboard, and when it exited.
//!
//! The pane keeps a history the owner can scroll with the wheel, and a text
//! selection the owner drives with the mouse and copies on release. The owner
//! can also read the screen and its history as plain text.
//!
//! The pane follows the prompt marks (OSC 133) its shell sends, so the owner
//! can see whether the shell waits at its prompt or runs a command, how the
//! last command ended, and read that command's output by line.

mod emulator;
mod encode;
mod graphics;
mod pane;
mod render;
mod shell;
mod splitter;

pub use graphics::{GraphicsFrame, Visible};
pub use pane::{
    CellSize, GraphicsSupport, PANE_READER_THREAD, Pane, PaneColors, PaneCommand, PaneError,
    PaneEvent, PaneSize,
};
pub use shell::{Finished, Lines, ShellPhase, ShellState, Spot};
