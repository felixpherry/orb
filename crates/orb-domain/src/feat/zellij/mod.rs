//! Zellij — where orb hands a thread's directory to a tool.
//!
//! A tool (a shell, lazygit or nvim) opens as a full-screen floating pane in
//! orb's zellij session, named after its directory and tool, and closes when
//! the tool exits. Asking for the same tool in the same directory again
//! focuses that pane, on whichever tab it is, instead of opening another.

pub mod zellij_cli;
pub mod zellij_service;
