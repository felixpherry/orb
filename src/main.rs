//! Binary entry point for orb. This is the only place that reads process state
//! (the environment).

use error_stack::Report;
use orb_tui::TuiRunError;

fn main() -> Result<(), Report<TuiRunError>> {
    orb_tui::run(std::env::vars_os().collect())
}
