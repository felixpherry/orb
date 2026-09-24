//! The frontend loop: draw, block on the next terminal event, apply its intent.

use error_stack::{Report, ResultExt};
use orb_domain::{AppState, IntentHandler};
use ratatui::Frame;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use ratatui::layout::{Constraint, Layout};
use wherror::Error;

use crate::keymap;

/// The frontend loop failed to draw a frame or read a terminal event.
#[derive(Debug, Error)]
#[error(debug)]
pub struct TuiRunError;

/// Run orb's TUI until the user quits. The terminal is restored on exit and
/// on panic.
///
/// # Errors
///
/// Returns [`TuiRunError`] if drawing a frame or reading a terminal event fails.
pub fn run() -> Result<(), Report<TuiRunError>> {
    ratatui::run(|terminal| -> std::io::Result<()> {
        let mut state = AppState::default();
        while !state.should_quit {
            terminal.draw(render)?;
            // Blocks until the next event; any event (key, resize, ...) redraws.
            if let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
                && let Some(intent) = keymap::intent_for(key)
            {
                IntentHandler::handle(&intent, &mut state);
            }
        }
        Ok(())
    })
    .change_context(TuiRunError)
}

/// Draw a blank screen with the mode line on the bottom row.
fn render(frame: &mut Frame) {
    let [_, mode_line] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(frame.area());
    frame.render_widget("NORMAL   q quit", mode_line);
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Cell;

    use super::render;

    #[rstest::rstest]
    fn mode_line_shows_normal_on_last_row() {
        // Given a 40x5 test terminal.
        let Ok(mut terminal) = Terminal::new(TestBackend::new(40, 5));

        // When drawing a frame.
        let Ok(_) = terminal.draw(render);

        // Then the bottom row is the mode line.
        let buffer = terminal.backend().buffer();
        let last_row: String = (0..buffer.area.width)
            .filter_map(|x| buffer.cell((x, buffer.area.height - 1)).map(Cell::symbol))
            .collect();
        assert!(last_row.starts_with("NORMAL"), "last row was '{last_row}'");
    }
}
