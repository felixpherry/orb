//! Zellij — where orb hands a thread's directory to a tool.
//!
//! A tool (a shell, lazygit or nvim) opens as a full-screen floating pane in
//! orb's zellij session, named after its directory and tool, and closes when
//! the tool exits. Asking for the same tool in the same directory again
//! focuses that pane, on whichever tab it is, instead of opening another.
//! A tool needs a selected thread or draft, whose directory it opens in.
//!
//! orb also asks zellij whether any client has orb's own pane focused, since
//! zellij tells a pane nothing when the user switches tab away from it, and
//! which tab orb's pane is on, so a notification's click can go back there.

pub mod validator;
pub mod zellij_cli;
pub mod zellij_service;
