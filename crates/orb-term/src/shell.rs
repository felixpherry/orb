//! What a pane's shell is doing, read from the prompt marks (OSC 133) it sends:
//! at its prompt, running a command (and which program), and how the last
//! command ended and which lines its output took. Lines are absolute: counted
//! from the pane's first line, so they stay put as the screen scrolls.

use crate::splitter::Mark;

/// A cell by absolute line and column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Spot {
    /// The absolute line: 0 is the pane's first line.
    pub line: i64,
    /// The column, 0 at the left edge.
    pub column: usize,
}

/// Where the shell is, between marks.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ShellPhase {
    /// No mark yet.
    #[default]
    Unknown,
    /// At its prompt; `input_at` is where typing starts, once the prompt
    /// ended.
    AtPrompt { input_at: Option<Spot> },
    /// Running a command whose output starts at `start_line`; `started` is
    /// the seq of the mark that started it.
    Running {
        program: Option<String>,
        start_line: i64,
        started: u64,
    },
}

/// The last command that ended: its exit status, its output's lines
/// (`end_line` not included), and the seq of the mark that started it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Finished {
    pub exit: Option<i32>,
    pub start_line: i64,
    pub end_line: i64,
    pub started: u64,
}

/// A snapshot of the pane's shell, and where its cursor is now.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ShellState {
    /// Whether the shell sent any prompt mark.
    pub marks_seen: bool,
    pub phase: ShellPhase,
    /// The last command that ended.
    pub last: Option<Finished>,
    /// Counts the marks; it goes up by one on every mark.
    pub seq: u64,
    pub cursor: Spot,
}

/// Lines of a pane's text, and whether their start had already scrolled out
/// of its history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lines {
    pub text: String,
    pub truncated: bool,
}

/// Follows the prompt marks a pane's shell sends. The newest mark wins, so a
/// shell started inside another takes over.
#[derive(Debug, Default)]
pub(crate) struct ShellTracker {
    marks_seen: bool,
    phase: ShellPhase,
    last: Option<Finished>,
    seq: u64,
}

impl ShellTracker {
    /// Applies `mark`, sent with the cursor at `cursor`. A command's output
    /// starts on the cursor's line when the cursor is at the line's start,
    /// else on the next one; it ends after the cursor's line when the cursor
    /// is past the line's start, which keeps a last line with no newline.
    pub(crate) fn mark(&mut self, mark: &Mark, cursor: Spot) {
        self.marks_seen = true;
        self.seq += 1;
        let line_from = |spot: Spot| match spot.column {
            0 => spot.line,
            _ => spot.line + 1,
        };
        self.phase = match mark {
            Mark::PromptStart => ShellPhase::AtPrompt { input_at: None },
            Mark::InputStart => ShellPhase::AtPrompt {
                input_at: Some(cursor),
            },
            Mark::CommandStart { program } => ShellPhase::Running {
                program: program.clone(),
                start_line: line_from(cursor),
                started: self.seq,
            },
            Mark::CommandEnd { exit } => {
                if let ShellPhase::Running {
                    start_line,
                    started,
                    ..
                } = self.phase
                {
                    self.last = Some(Finished {
                        exit: *exit,
                        start_line,
                        end_line: line_from(cursor).max(start_line),
                        started,
                    });
                }
                ShellPhase::AtPrompt { input_at: None }
            }
        };
    }

    /// Whether any mark arrived.
    pub(crate) fn marks_seen(&self) -> bool {
        self.marks_seen
    }

    /// The snapshot, with the cursor now at `cursor`.
    pub(crate) fn snapshot(&self, cursor: Spot) -> ShellState {
        ShellState {
            marks_seen: self.marks_seen,
            phase: self.phase.clone(),
            last: self.last,
            seq: self.seq,
            cursor,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Finished, ShellPhase, ShellTracker, Spot};
    use crate::splitter::Mark;

    /// The spot at `line`, `column`.
    fn spot(line: i64, column: usize) -> Spot {
        Spot { line, column }
    }

    /// A command start naming `program`.
    fn start(program: &str) -> Mark {
        Mark::CommandStart {
            program: Some(program.to_owned()),
        }
    }

    #[rstest::rstest]
    fn prompt_start_puts_the_shell_at_its_prompt() {
        // Given a shell that sent nothing yet.
        let mut shell = ShellTracker::default();

        // When it starts its prompt.
        shell.mark(&Mark::PromptStart, spot(0, 0));

        // Then it is at its prompt, with marks seen.
        let state = shell.snapshot(spot(0, 0));
        assert_eq!(
            (state.marks_seen, state.phase),
            (true, ShellPhase::AtPrompt { input_at: None }),
            "a prompt start means the shell waits at its prompt"
        );
    }

    #[rstest::rstest]
    fn input_start_notes_where_typing_begins() {
        // Given a shell drawing its prompt.
        let mut shell = ShellTracker::default();
        shell.mark(&Mark::PromptStart, spot(3, 0));

        // When its prompt ends at column 2.
        shell.mark(&Mark::InputStart, spot(3, 2));

        // Then typing starts there.
        assert_eq!(
            shell.snapshot(spot(3, 2)).phase,
            ShellPhase::AtPrompt {
                input_at: Some(spot(3, 2))
            },
            "the input mark notes where the command line starts"
        );
    }

    #[rstest::rstest]
    fn command_start_at_a_line_start_runs_from_that_line() {
        // Given a shell at its prompt.
        let mut shell = ShellTracker::default();
        shell.mark(&Mark::InputStart, spot(3, 2));

        // When a command starts at the start of line 4.
        shell.mark(&start("ls"), spot(4, 0));

        // Then its output runs from line 4.
        assert_eq!(
            shell.snapshot(spot(4, 0)).phase,
            ShellPhase::Running {
                program: Some("ls".to_owned()),
                start_line: 4,
                started: 2
            },
            "output starts on the line the cursor starts"
        );
    }

    #[rstest::rstest]
    fn command_start_mid_line_runs_from_the_next_line() {
        // Given a shell at its prompt.
        let mut shell = ShellTracker::default();
        shell.mark(&Mark::InputStart, spot(3, 2));

        // When a command starts with the cursor at the end of its line.
        shell.mark(&start("ls"), spot(3, 4));

        // Then its output runs from the next line.
        assert_eq!(
            shell.snapshot(spot(3, 4)).phase,
            ShellPhase::Running {
                program: Some("ls".to_owned()),
                start_line: 4,
                started: 2
            },
            "the command line itself isn't output"
        );
    }

    #[rstest::rstest]
    fn command_end_records_the_exit_and_lines() {
        // Given a command running from line 4.
        let mut shell = ShellTracker::default();
        shell.mark(&start("ls"), spot(4, 0));

        // When it ends with status 1 at the start of line 7.
        shell.mark(&Mark::CommandEnd { exit: Some(1) }, spot(7, 0));

        // Then its exit and lines 4 to 7 are kept, and the shell is back at
        // its prompt.
        let state = shell.snapshot(spot(7, 0));
        assert_eq!(
            (state.last, state.phase),
            (
                Some(Finished {
                    exit: Some(1),
                    start_line: 4,
                    end_line: 7,
                    started: 1
                }),
                ShellPhase::AtPrompt { input_at: None }
            ),
            "the ended command is recorded"
        );
    }

    #[rstest::rstest]
    fn command_end_mid_line_keeps_the_unfinished_line() {
        // Given a command running from line 4.
        let mut shell = ShellTracker::default();
        shell.mark(&start("printf"), spot(4, 0));

        // When it ends at column 4 of line 4, with no newline.
        shell.mark(&Mark::CommandEnd { exit: Some(0) }, spot(4, 4));

        // Then line 4 is part of its output.
        let last = shell.snapshot(spot(4, 4)).last;
        assert_eq!(
            last.map(|last| (last.start_line, last.end_line)),
            Some((4, 5)),
            "a last line with no newline is still output"
        );
    }

    #[rstest::rstest]
    fn command_end_without_a_start_records_nothing() {
        // Given a shell at its prompt.
        let mut shell = ShellTracker::default();
        shell.mark(&Mark::InputStart, spot(3, 2));

        // When an end comes with no command running.
        shell.mark(&Mark::CommandEnd { exit: Some(2) }, spot(4, 0));

        // Then no command is recorded, and the shell is at its prompt.
        let state = shell.snapshot(spot(4, 0));
        assert_eq!(
            (state.last, state.phase),
            (None, ShellPhase::AtPrompt { input_at: None }),
            "an end with no start isn't a command"
        );
    }

    #[rstest::rstest]
    fn every_mark_bumps_the_seq() {
        // Given a shell that sent nothing yet.
        let mut shell = ShellTracker::default();

        // When it sends a prompt, an input start, a command and its end.
        for mark in [
            Mark::PromptStart,
            Mark::InputStart,
            start("ls"),
            Mark::CommandEnd { exit: None },
        ] {
            shell.mark(&mark, spot(0, 0));
        }

        // Then the seq counted four marks.
        assert_eq!(shell.snapshot(spot(0, 0)).seq, 4, "each mark bumps the seq");
    }
}
