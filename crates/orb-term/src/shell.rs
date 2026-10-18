//! What a pane's shell is doing, read from the prompt marks (OSC 133) it sends:
//! at its prompt, running a command (and which program), and how the last
//! command ended and which lines its output took, and which lines held the
//! command lines it ran. Lines are absolute: counted from the pane's first
//! line, so they stay put as the screen scrolls.

use std::collections::VecDeque;
use std::ops::Range;

use crate::splitter::Mark;

/// How many of the newest command lines a pane remembers.
const COMMAND_LINES: usize = 16;

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

/// The last command that ended: its program (its first word), its exit
/// status, its output's lines (`end_line` not included), and the seq of the
/// mark that started it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finished {
    pub program: Option<String>,
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
    /// Whether the shell sent any command start mark: until it has, a
    /// command it runs may look like text left on the command line.
    pub starts_seen: bool,
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
    phase: ShellPhase,
    last: Option<Finished>,
    seq: u64,
    /// Whether a command start mark arrived.
    starts_seen: bool,
    /// The absolute lines of the newest command lines the shell ran, oldest
    /// first: from the line typing started on to the line its output starts.
    commands: VecDeque<Range<i64>>,
}

impl ShellTracker {
    /// Applies `mark`, sent with the cursor at `cursor`. A command's output
    /// starts on the cursor's line when the cursor is at the line's start,
    /// else on the next one; it ends after the cursor's line when the cursor
    /// is past the line's start, which keeps a last line with no newline.
    pub(crate) fn mark(&mut self, mark: &Mark, cursor: Spot) {
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
            Mark::CommandStart { program } => {
                self.starts_seen = true;
                let start_line = line_from(cursor);
                if let ShellPhase::AtPrompt {
                    input_at: Some(input),
                } = &self.phase
                {
                    if self.commands.len() == COMMAND_LINES {
                        self.commands.pop_front();
                    }
                    self.commands
                        .push_back(input.line..start_line.max(input.line + 1));
                }
                ShellPhase::Running {
                    program: program.clone(),
                    start_line,
                    started: self.seq,
                }
            }
            Mark::CommandEnd { exit } => {
                if let ShellPhase::Running {
                    program,
                    start_line,
                    started,
                } = &self.phase
                {
                    self.last = Some(Finished {
                        program: program.clone(),
                        exit: *exit,
                        start_line: *start_line,
                        end_line: line_from(cursor).max(*start_line),
                        started: *started,
                    });
                }
                ShellPhase::AtPrompt { input_at: None }
            }
        };
    }

    /// Whether any mark arrived.
    pub(crate) fn marks_seen(&self) -> bool {
        self.seq > 0
    }

    /// The absolute lines that hold command lines, with the cursor now at
    /// `cursor`: those of the newest commands the shell ran, and the line
    /// being typed at its prompt, from where typing started to the cursor's
    /// line.
    pub(crate) fn command_lines(&self, cursor: Spot) -> Vec<Range<i64>> {
        let typing = match &self.phase {
            ShellPhase::AtPrompt {
                input_at: Some(input),
            } => Some(input.line..cursor.line.max(input.line) + 1),
            _ => None,
        };
        self.commands.iter().cloned().chain(typing).collect()
    }

    /// The snapshot, with the cursor now at `cursor`.
    pub(crate) fn snapshot(&self, cursor: Spot) -> ShellState {
        ShellState {
            marks_seen: self.marks_seen(),
            starts_seen: self.starts_seen,
            phase: self.phase.clone(),
            last: self.last.clone(),
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
                    program: Some("ls".to_owned()),
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
    fn command_start_records_its_command_line() {
        // Given a shell whose prompt ended at column 2 of line 3.
        let mut shell = ShellTracker::default();
        shell.mark(&Mark::InputStart, spot(3, 2));

        // When a command typed over lines 3 and 4 starts at line 5.
        shell.mark(&start("ls"), spot(5, 0));

        // Then lines 3 and 4 hold its command line.
        assert_eq!(
            shell.command_lines(spot(5, 0)),
            vec![3..5],
            "the lines from the input mark to the output are command line"
        );
    }

    #[rstest::rstest]
    fn line_being_typed_counts_as_a_command_line() {
        // Given a shell whose prompt ended on line 3.
        let mut shell = ShellTracker::default();
        shell.mark(&Mark::InputStart, spot(3, 2));

        // When reading its command lines with the cursor on line 4.
        let lines = shell.command_lines(spot(4, 1));

        // Then lines 3 and 4 hold the line being typed.
        assert_eq!(
            lines,
            vec![3..5],
            "the unrun command line is command line too"
        );
    }

    #[rstest::rstest]
    fn only_the_newest_command_lines_are_kept() {
        // Given a shell that ran 20 commands, one a line.
        let mut shell = ShellTracker::default();
        for line in 0..20 {
            shell.mark(&Mark::InputStart, spot(line * 2, 2));
            shell.mark(&start("ls"), spot(line * 2 + 1, 0));
        }

        // When reading its command lines.
        let lines = shell.command_lines(spot(40, 0));

        // Then the newest 16 are kept, oldest first.
        assert_eq!(
            (lines.len(), lines.first().cloned()),
            (16, Some(8..9)),
            "older command lines drop out"
        );
    }

    #[rstest::rstest]
    fn prompt_marks_alone_leave_starts_unseen() {
        // Given a shell that sent nothing yet.
        let mut shell = ShellTracker::default();

        // When it sends a prompt start and an input start.
        shell.mark(&Mark::PromptStart, spot(0, 0));
        shell.mark(&Mark::InputStart, spot(0, 2));

        // Then no command start was seen.
        assert!(
            !shell.snapshot(spot(0, 2)).starts_seen,
            "a shell that only marks its prompt hasn't shown a command start"
        );
    }

    #[rstest::rstest]
    fn command_start_marks_starts_as_seen() {
        // Given a shell at its prompt.
        let mut shell = ShellTracker::default();
        shell.mark(&Mark::InputStart, spot(0, 2));

        // When a command starts.
        shell.mark(&start("ls"), spot(1, 0));

        // Then a command start was seen.
        assert!(
            shell.snapshot(spot(1, 0)).starts_seen,
            "a command start mark shows the shell sends them"
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
