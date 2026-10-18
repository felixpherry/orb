//! The command lines orb last typed into each pane, so `pane wait` in a shell
//! that sends no prompt marks can tell the echoed command line from the
//! output printed after it.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, PoisonError};

use crate::ids::PaneId;

/// How many of a pane's newest typed command lines are kept.
const KEPT: usize = 5;

/// The newest command lines orb typed into each pane, oldest first. Clones
/// share one map.
#[derive(Debug, Clone, Default)]
pub struct TypedLines(Arc<Mutex<HashMap<PaneId, VecDeque<String>>>>);

impl TypedLines {
    /// Remembers `line` as the newest command line typed into `pane`; a
    /// blank line is left out.
    pub(crate) fn remember(&self, pane: PaneId, line: &str) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        let mut map = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let lines = map.entry(pane).or_default();
        if lines.len() == KEPT {
            lines.pop_front();
        }
        lines.push_back(line.to_owned());
    }

    /// The newest command lines typed into `pane`, oldest first.
    pub(crate) fn recent(&self, pane: PaneId) -> Vec<String> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&pane)
            .map(|lines| lines.iter().cloned().collect())
            .unwrap_or_default()
    }
}

/// Whether `line` is the echo of one of the command lines in `typed`: it
/// ends with one, after other text such as the prompt. A line that is only
/// the command may be real output, so it isn't one.
pub(crate) fn echoed(line: &str, typed: &[String]) -> bool {
    let line = line.trim_end();
    typed
        .iter()
        .any(|command| line.len() > command.len() && line.ends_with(command.as_str()))
}

#[cfg(test)]
mod tests {
    use super::{TypedLines, echoed};
    use crate::ids::PaneId;

    #[rstest::rstest]
    fn only_the_newest_five_lines_are_kept() {
        // Given a pane orb typed seven command lines into.
        let typed = TypedLines::default();
        for n in 1..=7 {
            typed.remember(PaneId(1), &format!("echo {n}"));
        }

        // When reading its recent lines.
        let recent = typed.recent(PaneId(1));

        // Then the newest five are kept, oldest first.
        assert_eq!(
            recent,
            vec!["echo 3", "echo 4", "echo 5", "echo 6", "echo 7"],
            "older typed lines drop out"
        );
    }

    #[rstest::rstest]
    #[case::after_a_prompt("$ sleep 3; echo ZEBRA42", true)]
    #[case::output_alone("ZEBRA42", false)]
    #[case::command_alone("sleep 3; echo ZEBRA42", false)]
    #[case::trailing_spaces("~> sleep 3; echo ZEBRA42   ", true)]
    fn echoed_lines_end_with_a_typed_command_after_other_text(
        #[case] line: &str,
        #[case] expected: bool,
    ) {
        // Given orb typed `sleep 3; echo ZEBRA42`.
        let typed = vec!["sleep 3; echo ZEBRA42".to_owned()];

        // When checking whether the line is its echo.
        let echo = echoed(line, &typed);

        // Then only a line ending with it after other text is.
        assert_eq!(echo, expected, "{line:?} echoed: {expected}");
    }
}
