//! Binary entry point for orb: `orb -- <cmd…>` sets what the terminal pane
//! runs. This is the only place that reads process state (arguments, working
//! directory, environment).

use std::ffi::OsString;

use error_stack::{Report, ResultExt};
use orb_tui::TuiRunError;

fn main() -> Result<(), Report<TuiRunError>> {
    let pane_argv: Vec<OsString> = std::env::args_os()
        .skip_while(|arg| arg != "--")
        .skip(1)
        .collect();
    let cwd = std::env::current_dir().change_context(TuiRunError)?;
    orb_tui::run(pane_argv, cwd, std::env::vars_os().collect())
}
