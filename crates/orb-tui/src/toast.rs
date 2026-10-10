//! The message box, drawn like LazyVim's notifier in tokyonight-moon: a
//! rounded float just under the tab bar at the right edge of the right side,
//! titled ` Messages` in cyan for info or ` Error` in red. It is at least
//! 40 columns wide and grows with its text to 40% of the right side; longer
//! lines are cut with `…`, and each line break starts a new line.

use std::time::{Duration, Instant};

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
/// How long an info message shows.
const INFO_FOR: Duration = Duration::from_secs(3);
/// How long an error shows.
const ERROR_FOR: Duration = Duration::from_secs(5);
/// What the box says after orb copies a mouse selection.
const COPIED: &str = "Text copied to system clipboard";

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

/// The one message the box shows, if any. A message pops when its text is
/// new, shows for 3 s (info) or 5 s (an error), and is replaced by a newer
/// one. When orb clears an error or notice, its message goes too.
#[derive(Debug, Default)]
pub(crate) struct Toasts {
    shown: Option<Toast>,
    seen_error: Option<String>,
    seen_worktree_notice: Option<String>,
    seen_project_notice: Option<String>,
}

impl Toasts {
    /// Takes in the current error, worktree notice and project notice; pops
    /// any one whose text changed, the error over a notice when both did.
    pub(crate) fn observe(
        &mut self,
        error: Option<&str>,
        worktree_notice: Option<&str>,
        project_notice: Option<&str>,
        now: Instant,
    ) {
        Self::track(
            &mut self.shown,
            &mut self.seen_worktree_notice,
            worktree_notice,
            ToastKind::Info,
            now + INFO_FOR,
        );
        Self::track(
            &mut self.shown,
            &mut self.seen_project_notice,
            project_notice,
            ToastKind::Info,
            now + INFO_FOR,
        );
        Self::track(
            &mut self.shown,
            &mut self.seen_error,
            error,
            ToastKind::Error,
            now + ERROR_FOR,
        );
    }

    /// Pops the copy notice.
    pub(crate) fn copied(&mut self, now: Instant) {
        self.shown = Some(Toast::new(ToastKind::Info, COPIED, now + INFO_FOR));
    }

    /// The message showing at `now`.
    pub(crate) fn shown(&self, now: Instant) -> Option<&Toast> {
        self.shown.as_ref().filter(|toast| toast.until > now)
    }

    /// When the showing message hides.
    pub(crate) fn deadline(&self, now: Instant) -> Option<Instant> {
        self.shown(now).map(|toast| toast.until)
    }

    fn track(
        shown: &mut Option<Toast>,
        seen: &mut Option<String>,
        text: Option<&str>,
        kind: ToastKind,
        until: Instant,
    ) {
        if seen.as_deref() == text {
            return;
        }
        match text {
            Some(text) => *shown = Some(Toast::new(kind, text, until)),
            None => {
                if shown
                    .as_ref()
                    .is_some_and(|toast| toast.kind == kind && Some(&toast.text) == seen.as_ref())
                {
                    *shown = None;
                }
            }
        }
        *seen = text.map(str::to_owned);
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
    use std::time::{Duration, Instant};

    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    use super::{COPIED, Toast, ToastKind, Toasts, render};
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
        let corner = drawn(&buffer, 1).first().copied();
        assert!(
            top.contains("Messages") && corner.is_some_and(|x| buffer[(x, 1)].fg == CYAN),
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
        let corner = drawn(&buffer, 1).first().copied();
        assert!(
            top.contains("Error") && corner.is_some_and(|x| buffer[(x, 1)].fg == RED),
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
        let width = edge.first().zip(edge.last()).map(|(l, r)| r - l + 1);
        assert_eq!(
            width,
            Some(40),
            "a short message should still get a 40-column box"
        );
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
            edge.first().is_some_and(|x| *x >= right.x)
                && edge.last() == Some(&(right.right() - 2)),
            "the box should fit inside a narrow area: {edge:?}"
        );
    }

    /// A tracker that has just seen `error` at `at`.
    fn error_seen(error: &str, at: Instant) -> Toasts {
        let mut toasts = Toasts::default();
        toasts.observe(Some(error), None, None, at);
        toasts
    }

    #[rstest::rstest]
    fn new_error_still_shows_just_before_five_seconds() {
        // Given a new error observed.
        let at = Instant::now();
        let toasts = error_seen("boom", at);

        // When 4.9 s have passed.
        let shown = toasts.shown(at + Duration::from_millis(4900));

        // Then it still shows.
        assert_eq!(
            shown,
            Some(&Toast::new(
                ToastKind::Error,
                "boom",
                at + Duration::from_secs(5)
            )),
            "an error should show for 5 s"
        );
    }

    #[rstest::rstest]
    fn new_error_hides_after_five_seconds() {
        // Given a new error observed.
        let at = Instant::now();
        let toasts = error_seen("boom", at);

        // When 5 s have passed.
        let shown = toasts.shown(at + Duration::from_secs(5));

        // Then nothing shows.
        assert!(shown.is_none(), "an error should hide after 5 s");
    }

    #[rstest::rstest]
    fn new_notice_hides_after_three_seconds() {
        // Given a new notice observed.
        let at = Instant::now();
        let mut toasts = Toasts::default();
        toasts.observe(None, Some("pruned 2 worktrees"), None, at);

        // When 3 s have passed.
        let shown = toasts.shown(at + Duration::from_secs(3));

        // Then nothing shows.
        assert!(shown.is_none(), "a notice should hide after 3 s");
    }

    #[rstest::rstest]
    fn same_error_again_does_not_pop() {
        // Given an error that already timed out.
        let at = Instant::now();
        let mut toasts = error_seen("boom", at);
        let later = at + Duration::from_secs(6);

        // When the same error is observed again.
        toasts.observe(Some("boom"), None, None, later);

        // Then nothing shows.
        assert!(
            toasts.shown(later).is_none(),
            "the same error sent again should not bring the box back"
        );
    }

    #[rstest::rstest]
    fn error_replaces_showing_info() {
        // Given an info toast showing.
        let at = Instant::now();
        let mut toasts = Toasts::default();
        toasts.copied(at);

        // When an error is observed.
        toasts.observe(Some("boom"), None, None, at);

        // Then the error shows.
        assert_eq!(
            toasts.shown(at).map(|toast| toast.kind),
            Some(ToastKind::Error),
            "a newer message should replace the one showing"
        );
    }

    #[rstest::rstest]
    fn cleared_error_hides_its_box() {
        // Given an error showing.
        let at = Instant::now();
        let mut toasts = error_seen("boom", at);

        // When the error is observed cleared, as an orb key does.
        toasts.observe(None, None, None, at);

        // Then nothing shows.
        assert!(
            toasts.shown(at).is_none(),
            "clearing the error should hide its box"
        );
    }

    #[rstest::rstest]
    fn copy_shows_the_copy_notice() {
        // Given a copy event.
        let at = Instant::now();
        let mut toasts = Toasts::default();
        toasts.copied(at);

        // When asking what shows.
        let shown = toasts.shown(at);

        // Then it is the copy notice.
        assert_eq!(
            shown,
            Some(&Toast::new(
                ToastKind::Info,
                COPIED,
                at + Duration::from_secs(3)
            )),
            "a copy should show the copy notice for 3 s"
        );
    }

    #[rstest::rstest]
    fn new_project_notice_pops_as_info() {
        // Given no message showing.
        let at = Instant::now();
        let mut toasts = Toasts::default();

        // When a project notice is observed.
        toasts.observe(None, None, Some("Added project orb"), at);

        // Then it shows as info for 3 s.
        assert_eq!(
            toasts.shown(at),
            Some(&Toast::new(
                ToastKind::Info,
                "Added project orb",
                at + Duration::from_secs(3)
            )),
            "a new project notice should pop as info"
        );
    }

    #[rstest::rstest]
    fn error_wins_over_project_notice_in_same_frame() {
        // Given no message showing.
        let at = Instant::now();
        let mut toasts = Toasts::default();

        // When an error and a project notice change in the same frame.
        toasts.observe(Some("boom"), None, Some("Added project orb"), at);

        // Then the error shows.
        assert_eq!(
            toasts.shown(at).map(|toast| toast.kind),
            Some(ToastKind::Error),
            "the error should win over the notice"
        );
    }

    #[rstest::rstest]
    fn same_project_notice_after_clear_pops_again() {
        // Given a project notice seen, timed out, then cleared by a key.
        let at = Instant::now();
        let mut toasts = Toasts::default();
        toasts.observe(None, None, Some("Added project orb"), at);
        let later = at + Duration::from_secs(4);
        toasts.observe(None, None, None, later);

        // When the same notice is observed again.
        toasts.observe(None, None, Some("Added project orb"), later);

        // Then it shows again.
        assert!(
            toasts.shown(later).is_some(),
            "adding the same project again should pop the notice again"
        );
    }
}
