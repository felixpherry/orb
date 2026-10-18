//! How a pane's screen text is tidied before `pane read` sends it: trailing
//! whitespace cut from each line, blank lines dropped at both ends and
//! collapsed to one inside, then cut to the first `head` and last `tail` lines.

/// `text` with trailing whitespace cut from each line, blank lines dropped at
/// both ends, and each run of blank lines inside collapsed to one. A line of
/// only spaces counts as blank.
pub(crate) fn trim(text: &str) -> String {
    let mut kept: Vec<&str> = Vec::new();
    for line in text.lines().map(str::trim_end) {
        let after_blank = kept.last().is_none_or(|last| last.is_empty());
        if !line.is_empty() || !after_blank {
            kept.push(line);
        }
    }
    if kept.last().is_some_and(|last| last.is_empty()) {
        kept.pop();
    }
    kept.join("\n")
}

/// The first `head` lines of `text`, then the last `tail` of those; all of it
/// for `None`.
pub(crate) fn cut(text: &str, head: Option<usize>, tail: Option<usize>) -> String {
    let lines: Vec<&str> = text.lines().take(head.unwrap_or(usize::MAX)).collect();
    let skip = tail.map_or(0, |tail| lines.len().saturating_sub(tail));
    lines.get(skip..).unwrap_or_default().join("\n")
}

#[cfg(test)]
mod tests {
    use super::{cut, trim};

    #[rstest::rstest]
    #[case::trailing_spaces("a  \nb\t", "a\nb")]
    #[case::blank_ends("\n\na\nb\n\n\n", "a\nb")]
    #[case::blank_run_inside("a\n\n\n\nb", "a\n\nb")]
    #[case::spaces_only_line("a\n   \n  \nb", "a\n\nb")]
    #[case::all_blank("  \n\n \n", "")]
    fn trim_tidies_screen_text(#[case] text: &str, #[case] expected: &str) {
        // Given / When trimming the screen text.
        let trimmed = trim(text);

        // Then trailing spaces and extra blank lines are gone.
        assert_eq!(trimmed, expected, "{text:?} trims to {expected:?}");
    }

    #[rstest::rstest]
    #[case::head(Some(2), None, "1\n2")]
    #[case::tail(None, Some(2), "4\n5")]
    #[case::head_then_tail(Some(4), Some(2), "3\n4")]
    #[case::head_past_the_end(Some(9), None, "1\n2\n3\n4\n5")]
    #[case::neither(None, None, "1\n2\n3\n4\n5")]
    fn cut_keeps_head_then_tail(
        #[case] head: Option<usize>,
        #[case] tail: Option<usize>,
        #[case] expected: &str,
    ) {
        // Given five lines.
        let text = "1\n2\n3\n4\n5";

        // When cutting to the head, then the tail.
        let kept = cut(text, head, tail);

        // Then the first head lines are kept, then the last tail of those.
        assert_eq!(kept, expected, "head {head:?} tail {tail:?}");
    }
}
