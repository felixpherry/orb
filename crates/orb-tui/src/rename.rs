//! The rename box: a LazyVim-style input titled `Rename Session`, centred at
//! the top of the screen, where the user types orb's own name for a thread.

use orb_domain::feat::sidebar::state::Rename;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Position, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Widget};
use unicode_segmentation::UnicodeSegmentation;

use crate::sidebar::{BLUE1, FG, YELLOW};

/// The box's width, borders included, on a screen wide enough for it.
const WIDTH: u16 = 60;
/// Nerd Font's edit glyph (`nf-fa-edit`), before the name.
const EDIT: &str = "\u{f044}";

/// Draws the rename box over the top of `area` (the screen above the mode
/// line). A name too long for the box shows its part up to the cursor.
/// Returns where the terminal cursor goes.
pub(crate) fn render(rename: &Rename, area: Rect, buf: &mut Buffer) -> Position {
    let popup = {
        let width = area.width.min(WIDTH);
        Rect {
            x: area.x + (area.width - width) / 2,
            y: area.y + area.height.saturating_sub(3).min(2),
            width,
            height: area.height.min(3),
        }
    };
    Clear.render(popup, buf);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(YELLOW))
        .title(" Rename Session ")
        .title_style(Style::new().fg(YELLOW))
        .title_alignment(Alignment::Center);
    let inner = block.inner(popup);
    block.render(popup, buf);
    let prefix = vec![
        Span::raw(" "),
        Span::styled(EDIT, Style::new().fg(BLUE1)),
        Span::raw("  "),
    ];
    let prefix_width: usize = prefix.iter().map(Span::width).sum();
    let (shown, before) = visible(
        rename.input.text(),
        rename.input.cursor(),
        usize::from(inner.width).saturating_sub(prefix_width + 1),
    );
    let mut spans = prefix;
    spans.push(Span::styled(shown, Style::new().fg(FG)));
    Line::from(spans).render(inner, buf);
    let x = u16::try_from(prefix_width + before)
        .unwrap_or(u16::MAX)
        .min(inner.width.saturating_sub(1));
    Position::new(inner.x + x, inner.y)
}

/// The part of `text` to show when `room` columns fit before the cursor
/// (at grapheme `cursor`): everything, or a tail that keeps the cursor in
/// view. Returns it and its width before the cursor.
fn visible(text: &str, cursor: usize, room: usize) -> (String, usize) {
    let graphemes: Vec<&str> = text.graphemes(true).collect();
    let widths: Vec<usize> = graphemes
        .iter()
        .map(|grapheme| Span::raw(*grapheme).width())
        .collect();
    let mut before: usize = widths.iter().take(cursor).sum();
    let mut start = 0;
    while before > room && start < cursor {
        before -= widths.get(start).copied().unwrap_or_default();
        start += 1;
    }
    (graphemes.iter().skip(start).copied().collect(), before)
}

#[cfg(test)]
mod tests {
    use orb_domain::TextInput;
    use orb_domain::feat::sessions::state::ThreadId;
    use orb_domain::feat::sidebar::state::Rename;
    use ratatui::buffer::Buffer;
    use ratatui::layout::{Position, Rect};

    use super::render;
    use crate::sidebar::YELLOW;

    /// The rename box holding `text` drawn on a 100x10 screen.
    fn drawn(text: &str) -> (Buffer, Position) {
        let area = Rect::new(0, 0, 100, 10);
        let mut buf = Buffer::empty(area);
        let rename = Rename {
            thread: ThreadId(1),
            input: TextInput::new(text),
        };
        let cursor = render(&rename, area, &mut buf);
        (buf, cursor)
    }

    /// Row `y` of `buf` as text.
    fn row(buf: &Buffer, y: u16) -> String {
        (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()
    }

    #[rstest::rstest]
    fn box_is_sixty_columns_centred_two_rows_from_the_top() {
        // Given a 100-column screen.
        // When drawing the rename box.
        let (buf, _) = drawn("Fix the sidebar");

        // Then its corners are at columns 20 and 79 of row 2.
        assert_eq!(
            (buf[(20, 2)].symbol(), buf[(79, 2)].symbol()),
            ("╭", "╮"),
            "the box's top corners"
        );
    }

    #[rstest::rstest]
    fn box_border_is_yellow() {
        // Given a 100-column screen.
        // When drawing the rename box.
        let (buf, _) = drawn("Fix the sidebar");

        // Then its border is yellow.
        assert_eq!(buf[(20, 3)].fg, YELLOW, "the box's left border");
    }

    #[rstest::rstest]
    fn box_is_titled_rename_session() {
        // Given a 100-column screen.
        // When drawing the rename box.
        let (buf, _) = drawn("Fix the sidebar");

        // Then its top border carries the title.
        assert!(
            row(&buf, 2).contains(" Rename Session "),
            "the box's title: {:?}",
            row(&buf, 2)
        );
    }

    #[rstest::rstest]
    fn box_shows_the_name_after_the_edit_icon() {
        // Given a 100-column screen.
        // When drawing the rename box holding "Fix the sidebar".
        let (buf, _) = drawn("Fix the sidebar");

        // Then its line is the icon, then the name.
        assert!(
            row(&buf, 3).contains(" \u{f044}  Fix the sidebar"),
            "the box's line: {:?}",
            row(&buf, 3)
        );
    }

    #[rstest::rstest]
    fn cursor_sits_after_the_name() {
        // Given a 100-column screen.
        // When drawing the rename box holding "Fix".
        let (_, cursor) = drawn("Fix");

        // Then the cursor is past the border, the icon's four columns and the
        // three letters.
        assert_eq!(cursor, Position::new(28, 3), "the cursor after the name");
    }

    #[rstest::rstest]
    fn long_name_keeps_the_cursor_in_the_box() {
        // Given a name wider than the box.
        // When drawing it.
        let (_, cursor) = drawn(&"x".repeat(80));

        // Then the cursor stays inside the right border.
        assert_eq!(cursor, Position::new(78, 3), "the cursor at the box's end");
    }
}
