//! The which-key popup, drawn like LazyVim's default which-key (the helix
//! preset) in tokyonight-moon: a rounded float in the bottom-right corner,
//! sitting on the mode line, with the pending keys in its top border and one
//! `key ➜ icon desc` row per next key, groups as `+name`. The rows are in
//! which-key's order (letters and digits before symbols, lowercase before its
//! capital), and `esc close  ⌫ back` sits on the last row. The popup is as
//! tall as its rows; on a shorter screen the rows that don't fit are cut off.

use orb_domain::Intent;
use orb_domain::feat::zellij::zellij_service::Tool;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Widget};
use ratatui_which_key::{Key as _, NodeResult};

use crate::keymap::Keys;
use crate::sidebar::{
    BG_DARK, BLUE, BLUE1, BORDER, COMMENT, CYAN, DARK5, FOLDER, FOLDER_OPEN, GREEN, MAGENTA,
    ORANGE, RED, YELLOW, kind_look,
};

/// One row of the popup: a next key and what it does.
struct Entry {
    code: KeyCode,
    key: String,
    desc: String,
    icon: &'static str,
    colour: Color,
    group: bool,
}

/// Draws the popup for the pending sequence inside `area`, the screen above
/// the mode line. Nothing is drawn without a pending sequence, or when
/// `area` is too short for a row between the borders and the foot.
pub(crate) fn render(keys: &Keys, area: Rect, buf: &mut Buffer) {
    if !keys.active && keys.current_sequence.is_empty() {
        return;
    }
    let entries = entries(keys);
    if entries.is_empty() || area.height < 4 {
        return;
    }
    let key_width = entries.iter().map(|e| width(&e.key)).max().unwrap_or(1);
    let inner_width = entries
        .iter()
        .map(|e| row_width(e, key_width))
        .max()
        .unwrap_or(0)
        .max(help().width());
    let popup = {
        let width = to_u16(inner_width + 4).min(area.width);
        let height = to_u16(entries.len() + 3).min(area.height);
        Rect::new(
            area.right().saturating_sub(width + 1),
            area.bottom().saturating_sub(height),
            width,
            height,
        )
    };
    float(popup, buf);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(BORDER))
        .title(Line::from(span(format!(" {} ", path(keys)), ORANGE)));
    let inner = block.inner(popup);
    block.render(popup, buf);
    let x = inner.x + 1;
    let max_width = inner.width.saturating_sub(1);
    let rows = inner.height.saturating_sub(1);
    for (entry, y) in entries.iter().zip(inner.y..inner.y + rows) {
        buf.set_line(x, y, &row(entry, key_width), max_width);
    }
    buf.set_line(x, inner.bottom() - 1, &help(), max_width);
}

/// The keys under the pending sequence, in which-key's order: letters and
/// digits before symbols, lowercase before its capital.
fn entries(keys: &Keys) -> Vec<Entry> {
    let path = &keys.current_sequence;
    let scope = keys.scope();
    let mut entries: Vec<Entry> = keys
        .keymap()
        .children_at_path(path, scope)
        .unwrap_or_default()
        .into_iter()
        .map(|binding| {
            let full: Vec<KeyEvent> = path.iter().copied().chain([binding.key]).collect();
            let key = key_name(&binding.key);
            match keys.keymap().navigate(&full, scope) {
                Some(NodeResult::Leaf { action }) => {
                    let (icon, colour) = look(&action);
                    Entry {
                        code: binding.key.code,
                        key,
                        desc: binding.description,
                        icon,
                        colour,
                        group: false,
                    }
                }
                _ => Entry {
                    code: binding.key.code,
                    key,
                    desc: format!("+{}", binding.description),
                    icon: FOLDER,
                    colour: BLUE,
                    group: true,
                },
            }
        })
        .collect();
    entries.sort_by_key(|e| {
        let (symbol, upper) = match e.code {
            KeyCode::Char(c) => (!c.is_alphanumeric(), c.is_uppercase()),
            _ => (true, false),
        };
        (symbol, e.key.to_lowercase(), upper)
    });
    entries
}

/// orb's names for keys, as LazyVim writes them.
fn key_name(key: &KeyEvent) -> String {
    match key.display().as_str() {
        "Space" => "␣".to_owned(),
        "Tab" => "<Tab>".to_owned(),
        "Enter" => "<CR>".to_owned(),
        other => other.to_owned(),
    }
}

/// The icon and its colour for an intent (mini.icons style).
fn look(intent: &Intent) -> (&'static str, Color) {
    match intent {
        Intent::NewSession | Intent::NewSibling => ("\u{f067}", GREEN),
        Intent::AddProject | Intent::OpenGroup => (FOLDER_OPEN, BLUE),
        Intent::FilterProjects => ("\u{f0b0}", CYAN),
        Intent::PickModel => ("\u{f0e7}", MAGENTA),
        Intent::PickPermission => ("\u{f023}", YELLOW),
        Intent::ChangeWorkspace => ("\u{f1bb}", GREEN),
        Intent::SwitchBranch => ("\u{e725}", ORANGE),
        Intent::OpenTool(Tool::Shell) => ("\u{f120}", CYAN),
        Intent::OpenTool(Tool::Lazygit) => ("\u{f1d3}", RED),
        Intent::OpenTool(Tool::Nvim) => ("\u{e62b}", GREEN),
        Intent::ToggleSidebar => ("\u{f0db}", BLUE1),
        Intent::SelectFirst => ("\u{f062}", BLUE),
        Intent::NewGroup(kind) => kind_look(*kind),
        Intent::CloseGroup => (FOLDER, BLUE),
        _ => ("\u{f111}", DARK5),
    }
}

/// The pending path, e.g. `␣` or `g`.
fn path(keys: &Keys) -> String {
    keys.current_sequence
        .iter()
        .map(key_name)
        .collect::<Vec<_>>()
        .join(" ")
}

/// `key ➜ icon desc`, the key right-aligned to `key_width`.
fn row(entry: &Entry, key_width: usize) -> Line<'_> {
    let desc = if entry.group { BLUE } else { MAGENTA };
    Line::from(vec![
        span(format!("{:>key_width$}", entry.key), CYAN),
        span(" ➜ ", COMMENT),
        span(format!("{} ", entry.icon), entry.colour),
        span(entry.desc.as_str(), desc),
    ])
}

fn row_width(entry: &Entry, key_width: usize) -> usize {
    key_width + 3 + 2 + width(&entry.desc)
}

/// `esc close  ⌫ back`.
fn help() -> Line<'static> {
    Line::from(vec![
        span("esc ", CYAN),
        span("close  ", COMMENT),
        span("⌫ ", CYAN),
        span("back", COMMENT),
    ])
}

/// A floating box on `BG_DARK`.
fn float(area: Rect, buf: &mut Buffer) {
    Clear.render(area, buf);
    buf.set_style(area, Style::new().bg(BG_DARK));
}

fn span<'a>(text: impl Into<std::borrow::Cow<'a, str>>, fg: Color) -> Span<'a> {
    Span::styled(text, Style::new().fg(fg))
}

/// Display width of `text`.
fn width(text: &str) -> usize {
    Span::raw(text).width()
}

fn to_u16(n: usize) -> u16 {
    u16::try_from(n).unwrap_or(u16::MAX)
}

#[cfg(test)]
mod tests {
    use orb_domain::Intent;
    use ratatui::buffer::{Buffer, Cell};
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::layout::Rect;
    use ratatui::style::Color;
    use ratatui_which_key::Keymap;

    use super::render;
    use crate::keymap::{KeyCategory, Keys, Scope, keymap, press};
    use crate::sidebar::{BLUE2, FOLDER, FOLDER_OPEN, GREEN1, PURPLE};

    const SCREEN: Rect = Rect::new(0, 0, 80, 20);

    /// `keys` with Space pressed.
    fn leader(mut keys: Keys) -> Keys {
        press(
            &mut keys,
            KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
        );
        keys
    }

    /// orb's keymap on a thread with Space pressed.
    fn leader_on_thread() -> Keys {
        leader(Keys::new(keymap(), Scope::Sidebar))
    }

    /// A buffer the size of `SCREEN` with the popup drawn inside `area`.
    fn draw(keys: &Keys, area: Rect) -> Buffer {
        let mut buffer = Buffer::empty(SCREEN);
        render(keys, area, &mut buffer);
        buffer
    }

    /// Each row of `buffer` as text.
    fn lines(buffer: &Buffer) -> Vec<String> {
        buffer
            .area
            .rows()
            .map(|row| {
                row.columns()
                    .filter_map(|cell| buffer.cell(cell).map(Cell::symbol))
                    .collect::<String>()
            })
            .collect()
    }

    #[rstest::rstest]
    fn popup_rows_are_in_which_key_order() {
        // Given a keymap with `␣/`, `␣B`, `␣b`, `␣a` and `␣1`, and Space pressed.
        let keys = {
            let mut km = Keymap::new();
            for key in ["/", "B", "b", "a", "1"] {
                km.bind(
                    &format!("<leader>{key}"),
                    Intent::NewSession,
                    KeyCategory::Sessions,
                    Scope::Sidebar,
                );
            }
            leader(Keys::new(km, Scope::Sidebar))
        };

        // When drawing the popup.
        let buffer = draw(&keys, SCREEN);

        // Then the row keys read `1 a b B /` top to bottom.
        let order: Vec<String> = lines(&buffer)
            .iter()
            .filter_map(|line| line.split_once('➜'))
            .map(|(key, _)| key.trim_matches([' ', '│']).to_owned())
            .collect();
        assert_eq!(order, ["1", "a", "b", "B", "/"], "the row keys");
    }

    #[rstest::rstest]
    fn popup_top_border_shows_the_pending_path() {
        // Given Space pressed on a thread.
        let keys = leader_on_thread();

        // When drawing the popup.
        let buffer = draw(&keys, SCREEN);

        // Then the top border carries ` ␣ `.
        let lines = lines(&buffer);
        let top = lines.iter().find(|line| line.contains('╭'));
        assert!(
            top.is_some_and(|line| line.contains("╭ ␣ ")),
            "the top border was {top:?}"
        );
    }

    #[rstest::rstest]
    fn leaf_row_reads_key_arrow_icon_desc() {
        // Given Space pressed on a thread.
        let keys = leader_on_thread();

        // When drawing the popup.
        let buffer = draw(&keys, SCREEN);

        // Then the `n` row reads `n ➜ <plus> new session`.
        let lines = lines(&buffer);
        assert!(
            lines
                .iter()
                .any(|line| line.contains("│ n ➜ \u{f067} new session")),
            "the screen was {lines:#?}"
        );
    }

    #[rstest::rstest]
    fn group_row_reads_plus_name() {
        // Given a keymap with a `␣x` group named `extra`, and Space pressed.
        let keys = {
            let mut km = Keymap::new();
            km.describe_group("<leader>x", "extra");
            km.bind(
                "<leader>xa",
                Intent::NewSession,
                KeyCategory::Sessions,
                Scope::Sidebar,
            );
            leader(Keys::new(km, Scope::Sidebar))
        };

        // When drawing the popup.
        let buffer = draw(&keys, SCREEN);

        // Then the `x` row reads `x ➜ <folder> +extra`.
        let lines = lines(&buffer);
        assert!(
            lines
                .iter()
                .any(|line| line.contains("│ x ➜ \u{f07b} +extra")),
            "the screen was {lines:#?}"
        );
    }

    #[rstest::rstest]
    fn popup_foot_shows_the_help() {
        // Given Space pressed on a thread.
        let keys = leader_on_thread();

        // When drawing the popup.
        let buffer = draw(&keys, SCREEN);

        // Then the row above the bottom border reads `esc close  ⌫ back`.
        let lines = lines(&buffer);
        let foot = lines
            .iter()
            .position(|line| line.contains('╰'))
            .and_then(|bottom| lines.get(bottom - 1));
        assert!(
            foot.is_some_and(|line| line.contains("│ esc close  ⌫ back")),
            "the foot was {foot:?}"
        );
    }

    #[rstest::rstest]
    fn popup_sits_bottom_right_on_the_area_bottom() {
        // Given Space pressed on a thread, and an area smaller than the screen.
        let keys = leader_on_thread();
        let area = Rect::new(0, 0, 60, 16);

        // When drawing the popup inside the area.
        let buffer = draw(&keys, area);

        // Then its bottom-right corner is on the area's last row, one column
        // in from its right edge.
        let corner = buffer
            .cell((area.right() - 2, area.bottom() - 1))
            .map(Cell::symbol);
        assert_eq!(corner, Some("╯"), "the bottom-right corner");
    }

    #[rstest::rstest]
    fn too_tall_popup_is_cut_off_keeping_the_foot() {
        // Given Space pressed on a thread, and an area 7 rows tall.
        let keys = leader_on_thread();
        let area = Rect::new(0, 0, 80, 7);

        // When drawing the popup inside the area.
        let buffer = draw(&keys, area);

        // Then the box fills the 7 rows and its last inner row is the help.
        let lines = lines(&buffer);
        let row_has = |y: usize, text: &str| lines.get(y).is_some_and(|line| line.contains(text));
        assert!(
            row_has(0, "╭") && row_has(5, "esc close  ⌫ back") && row_has(6, "╰"),
            "the screen was {lines:#?}"
        );
    }

    #[rstest::rstest]
    fn no_popup_without_a_pending_sequence() {
        // Given orb's keymap with nothing pressed.
        let keys = Keys::new(keymap(), Scope::Sidebar);

        // When drawing the popup.
        let buffer = draw(&keys, SCREEN);

        // Then nothing is drawn.
        let lines = lines(&buffer);
        assert!(
            !lines.iter().any(|line| line.contains('╭')),
            "the screen was {lines:#?}"
        );
    }

    #[rstest::rstest]
    fn leader_popup_shows_g_as_the_group_group() {
        // Given Space pressed on a thread.
        let keys = leader_on_thread();

        // When drawing the popup.
        let buffer = draw(&keys, SCREEN);

        // Then the `g` row reads `g ➜ <folder> +group`.
        let lines = lines(&buffer);
        assert!(
            lines
                .iter()
                .any(|line| line.contains("│ g ➜ \u{f07b} +group")),
            "the screen was {lines:#?}"
        );
    }

    #[rstest::rstest]
    #[case('f', "\u{f126}", GREEN1)]
    #[case('r', "\u{f0c3}", BLUE2)]
    #[case('l', "\u{f02d}", PURPLE)]
    fn new_group_rows_show_their_kind_icon(
        #[case] pressed: char,
        #[case] icon: &str,
        #[case] colour: Color,
    ) {
        // Given Space and `g` pressed on a thread.
        let keys = {
            let mut keys = leader_on_thread();
            press(
                &mut keys,
                KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE),
            );
            keys
        };

        // When drawing the popup.
        let buffer = draw(&keys, SCREEN);

        // Then the kind's row has the kind's icon in its colour.
        let row = format!("{pressed} ➜ ");
        let lines = lines(&buffer);
        let cell = buffer
            .area
            .positions()
            .filter(|at| {
                lines
                    .get(usize::from(at.y))
                    .is_some_and(|l| l.contains(&row))
            })
            .filter_map(|at| buffer.cell(at))
            .find(|cell| cell.symbol() == icon)
            .map(|cell| (cell.symbol().to_owned(), cell.fg));
        assert_eq!(
            cell,
            Some((icon.to_owned(), colour)),
            "the icon on the {pressed} row"
        );
    }

    #[rstest::rstest]
    #[case(Intent::OpenGroup, FOLDER_OPEN, "open group")]
    #[case(Intent::CloseGroup, FOLDER, "close group")]
    fn fold_intents_show_folder_icons(
        #[case] intent: Intent,
        #[case] icon: &str,
        #[case] label: &str,
    ) {
        // Given a keymap with `␣x` bound to the intent, and Space pressed.
        let keys = {
            let mut km = Keymap::new();
            km.bind("<leader>x", intent, KeyCategory::Navigation, Scope::Sidebar);
            leader(Keys::new(km, Scope::Sidebar))
        };

        // When drawing the popup.
        let buffer = draw(&keys, SCREEN);

        // Then the `x` row shows the folder icon.
        let lines = lines(&buffer);
        let row = format!("│ x ➜ {icon} {label}");
        assert!(
            lines.iter().any(|line| line.contains(&row)),
            "the screen was {lines:#?}"
        );
    }

    #[rstest::rstest]
    fn new_sibling_shows_the_plus_icon() {
        // Given a keymap with `␣x` bound to NewSibling, and Space pressed.
        let keys = {
            let mut km = Keymap::new();
            km.bind(
                "<leader>x",
                Intent::NewSibling,
                KeyCategory::Sessions,
                Scope::Sidebar,
            );
            leader(Keys::new(km, Scope::Sidebar))
        };

        // When drawing the popup.
        let buffer = draw(&keys, SCREEN);

        // Then the `x` row shows the plus icon.
        let lines = lines(&buffer);
        assert!(
            lines
                .iter()
                .any(|line| line.contains("│ x ➜ \u{f067} new sibling")),
            "the screen was {lines:#?}"
        );
    }
}
