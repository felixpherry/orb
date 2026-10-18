//! The command lines orb last typed into each pane, so `pane wait` can tell
//! a line echoed back, by a shell that sends no prompt marks or by a program
//! such as a REPL, from the output printed after it.

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

/// The characters a prompt may end with, before the typed command.
const PROMPT_ENDS: &[char] = &['>', '$', '#', '%', ':', ')', ']'];

/// Whether `line` is the echo of one of the command lines in `typed`: it is
/// the command, or ends with it after a prompt, text that ends with a blank
/// or one of [`PROMPT_ENDS`]. A line ending with the command in the middle
/// of a word (`Ready` after typing `y`) is output.
pub(crate) fn echoed(line: &str, typed: &[String]) -> bool {
    let line = line.trim();
    typed.iter().any(|command| {
        line.strip_suffix(command.as_str()).is_some_and(|before| {
            before.is_empty()
                || before.ends_with(|end: char| end.is_whitespace() || PROMPT_ENDS.contains(&end))
        })
    })
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
    #[case::command_alone("sleep 3; echo ZEBRA42", true)]
    #[case::trailing_spaces("~> sleep 3; echo ZEBRA42   ", true)]
    #[case::prompt_without_a_blank("~>sleep 3; echo ZEBRA42", true)]
    #[case::after_other_words("ran sleep 3; echo ZEBRA42", true)]
    fn echoed_lines_are_a_typed_command_after_a_prompt(#[case] line: &str, #[case] expected: bool) {
        // Given orb typed `sleep 3; echo ZEBRA42`.
        let typed = vec!["sleep 3; echo ZEBRA42".to_owned()];

        // When checking whether the line is its echo.
        let echo = echoed(line, &typed);

        // Then only the command, or a line ending with it after a prompt,
        // is.
        assert_eq!(echo, expected, "{line:?} echoed: {expected}");
    }

    #[rstest::rstest]
    #[case::word_ending_in_a_typed_letter("Ready", "y")]
    #[case::word_ending_in_a_typed_command("tools", "ls")]
    fn a_word_ending_with_a_typed_command_is_output(#[case] line: &str, #[case] command: &str) {
        // Given orb typed `command`.
        let typed = vec![command.to_owned()];

        // When checking whether a line whose last word ends with it is its
        // echo.
        let echo = echoed(line, &typed);

        // Then it isn't: the command ends mid-word.
        assert!(!echo, "{line:?} isn't the echo of {command:?}");
    }
}
