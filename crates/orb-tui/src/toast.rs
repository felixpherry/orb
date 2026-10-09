//! The message box, drawn like LazyVim's notifier in tokyonight-moon: a
//! rounded float just under the tab bar at the right edge of the right side,
//! titled ` Messages` in cyan for info or ` Error` in red. It is at least
//! 40 columns wide and grows with its text to 40% of the right side; longer
//! lines are cut with `…`, and each line break starts a new line.

use std::time::Instant;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph, Widget};

use crate::session_picker::cut_right;
use crate::sidebar::{BG_DARK, CYAN, FAILED_ICON, FG, RED};

/// The narrowest the box gets, whatever its text.
const MIN_WIDTH: u16 = 40;
/// Before the info title (`nf-fa-info_circle`).
const INFO_ICON: &str = "\u{f05a}";

/// What a message is about, which sets the box's title and colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToastKind {
    /// A notice, such as a copied selection or pruned worktrees.
    Info,
    /// Something that went wrong.
    Error,
}

/// One message in the box, shown until `until`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Toast {
    kind: ToastKind,
    text: String,
    until: Instant,
}

impl Toast {
    pub(crate) fn new(kind: ToastKind, text: impl Into<String>, until: Instant) -> Self {
        Self {
            kind,
            text: text.into(),
            until,
        }
    }
}

/// Draws `toast` one row below the top of `right`, its right edge one column
/// in from the right side's. Nothing is drawn when `right` is too small for
/// a line of text.
pub(crate) fn render(toast: &Toast, right: Rect, buf: &mut Buffer) {
    let (icon, title, colour) = match toast.kind {
        ToastKind::Info => (INFO_ICON, "Messages", CYAN),
        ToastKind::Error => (FAILED_ICON, "Error", RED),
    };
    let lines: Vec<&str> = toast.text.split('\n').collect();
    let area = {
        let text_width = lines
            .iter()
            .map(|line| Line::raw(*line).width())
            .max()
            .unwrap_or(0);
        let max_width = MIN_WIDTH.max(right.width * 2 / 5);
        let width = u16::try_from(text_width + 4)
            .unwrap_or(u16::MAX)
            .clamp(MIN_WIDTH, max_width)
            .min(right.width.saturating_sub(1));
        let height = u16::try_from(lines.len() + 2)
            .unwrap_or(u16::MAX)
            .min(right.height.saturating_sub(1));
        Rect::new(
            right.right().saturating_sub(width + 1),
            right.y + 1,
            width,
            height,
        )
    };
    if area.width < 5 || area.height < 3 {
        return;
    }
    Clear.render(area, buf);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(colour))
        .title(Line::styled(format!(" {icon} {title} "), Style::new().fg(colour)).centered())
        .padding(Padding::horizontal(1))
        .style(Style::new().bg(BG_DARK));
    let text_width = usize::from(area.width - 4);
    let text: Vec<Line> = lines
        .iter()
        .map(|line| Line::styled(cut_right(line, text_width), Style::new().fg(FG)))
        .collect();
    Paragraph::new(text).block(block).render(area, buf);
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    use super::{Toast, ToastKind, render};
    use crate::sidebar::{CYAN, RED};

    const RIGHT: Rect = Rect::new(10, 0, 120, 30);

    fn draw(kind: ToastKind, text: &str, right: Rect) -> Buffer {
        let mut buffer = Buffer::empty(Rect::new(0, 0, right.right(), right.bottom()));
        render(&Toast::new(kind, text, Instant::now()), right, &mut buffer);
        buffer
    }

    /// The text of row `y`.
    fn row(buffer: &Buffer, y: u16) -> String {
        (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol())
            .collect()
    }

    /// The columns of row `y` that hold something other than a space.
    fn drawn(buffer: &Buffer, y: u16) -> Vec<u16> {
        (0..buffer.area.width)
            .filter(|x| buffer[(*x, y)].symbol() != " ")
            .collect()
    }

    #[rstest::rstest]
    fn info_toast_has_cyan_messages_title() {
        // Given an info toast.
        // When drawn.
        let buffer = draw(ToastKind::Info, "pruned 2 worktrees", RIGHT);

        // Then the top edge is cyan and titled Messages.
        let top = row(&buffer, 1);
        let corner = drawn(&buffer, 1)[0];
        assert!(
            top.contains("Messages") && buffer[(corner, 1)].fg == CYAN,
            "info box should be cyan with a Messages title: {top:?}"
        );
    }

    #[rstest::rstest]
    fn error_toast_has_red_error_title() {
        // Given an error toast.
        // When drawn.
        let buffer = draw(ToastKind::Error, "claude --bg failed", RIGHT);

        // Then the top edge is red and titled Error.
        let top = row(&buffer, 1);
        let corner = drawn(&buffer, 1)[0];
        assert!(
            top.contains("Error") && buffer[(corner, 1)].fg == RED,
            "error box should be red with an Error title: {top:?}"
        );
    }

    #[rstest::rstest]
    fn short_message_gets_a_forty_column_box() {
        // Given a short message.
        // When drawn.
        let buffer = draw(ToastKind::Info, "hi", RIGHT);

        // Then the box is 40 columns wide.
        let edge = drawn(&buffer, 1);
        let width = edge[edge.len() - 1] - edge[0] + 1;
        assert_eq!(width, 40, "a short message should still get a 40-column box");
    }

    #[rstest::rstest]
    fn long_line_is_cut_at_forty_percent() {
        // Given a line longer than 40% of a 120-column area.
        // When drawn.
        let buffer = draw(ToastKind::Info, &"x".repeat(100), RIGHT);

        // Then the box is 48 columns wide and its line ends with "…".
        let text = row(&buffer, 2);
        let expected = format!("│ {}… │", "x".repeat(43));
        assert!(
            text.contains(&expected),
            "the line should be cut with … at 40% of the area: {text:?}"
        );
    }

    #[rstest::rstest]
    fn line_break_starts_a_new_line() {
        // Given a message with a line break.
        // When drawn.
        let buffer = draw(ToastKind::Info, "first\nsecond", RIGHT);

        // Then each half is on its own row, and the box closes after them.
        let rows = [row(&buffer, 2), row(&buffer, 3), row(&buffer, 4)];
        assert!(
            rows[0].contains("first") && rows[1].contains("second") && rows[2].contains('╰'),
            "a line break should give two text lines: {rows:?}"
        );
    }

    #[rstest::rstest]
    fn box_sits_under_the_tab_bar_one_column_from_the_right() {
        // Given any toast.
        // When drawn.
        let buffer = draw(ToastKind::Info, "hi", RIGHT);

        // Then its top-right corner is on row 1, one column in from the right.
        let corner = (RIGHT.right() - 2, RIGHT.y + 1);
        assert!(
            buffer[corner].symbol() == "╮" && drawn(&buffer, RIGHT.y).is_empty(),
            "the box should start one row down and one column in from the right"
        );
    }

    #[rstest::rstest]
    fn narrow_area_keeps_box_inside() {
        // Given a 30-column area.
        let right = Rect::new(50, 0, 30, 10);

        // When drawn.
        let buffer = draw(ToastKind::Error, "something went wrong here", right);

        // Then nothing is drawn left of the area.
        let edge = drawn(&buffer, 1);
        assert!(
            edge.first().is_some_and(|x| *x >= right.x) && edge.last() == Some(&(right.right() - 2)),
            "the box should fit inside a narrow area: {edge:?}"
        );
    }
}
