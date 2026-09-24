//! Binary entry point for orb.

use error_stack::Report;
use orb_tui::TuiRunError;

fn main() -> Result<(), Report<TuiRunError>> {
    orb_tui::run()
}
