//! PROTOTYPE (throwaway, keep off main): LazyVim-style which-key popups drawn
//! from the real keymap in place of ratatui-which-key's own.
//!
//! `ORB_WHICHKEY_VARIANT` picks the look; while a popup is open, `←`/`→` flip
//! between them without ending the sequence.
//! - `current`: ratatui-which-key's widget, as orb draws it today
//! - `helix`: LazyVim's default preset: a tall, narrow popup in the bottom
//!   right, `key ➜ icon desc` rows, the path in the top border, help at the foot
//! - `classic`: which-key's classic preset: a full-width panel docked above
//!   the mode line, a grid of `key ➜ icon desc`, path and help on its last row
//! - `modern`: which-key's modern preset, grouped: a floating box centred at the
//!   bottom with one column per section under a heading, keys in orange

use std::fmt::Write as _;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use orb_domain::Intent;
use orb_domain::feat::zellij::zellij_service::Tool;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Widget};
use ratatui_which_key::{Key as _, NodeResult, WhichKey};

use crate::keymap::Keys;
use crate::sidebar::{
    BG_DARK, BLUE, BLUE1, BORDER, COMMENT, CYAN, DARK5, FG_DARK, GREEN, MAGENTA, ORANGE, YELLOW,
};

const RED: Color = Color::Rgb(0xff, 0x75, 0x7f);
const VARIANTS: [&str; 4] = ["current", "helix", "classic", "modern"];
static VARIANT: AtomicUsize = AtomicUsize::new(usize::MAX);

fn env_variant() -> Option<usize> {
    static ENV: OnceLock<Option<usize>> = OnceLock::new();
    *ENV.get_or_init(|| {
        let name = std::env::var("ORB_WHICHKEY_VARIANT").ok()?;
        Some(VARIANTS.iter().position(|v| *v == name).unwrap_or(1))
    })
}

fn current() -> usize {
    match VARIANT.load(Ordering::Relaxed) {
        usize::MAX => env_variant().unwrap_or(0),
        v => v,
    }
}

/// Whether the prototype is switched on.
pub(crate) fn active() -> bool {
    env_variant().is_some() || VARIANT.load(Ordering::Relaxed) != usize::MAX
}

/// Display width of `text`.
fn width(text: &str) -> usize {
    Span::raw(text).width()
}

/// `←`/`→` while a popup is open flip the variant; returns whether it took
/// the key.
pub(crate) fn flip(keys: &Keys, key: KeyEvent) -> bool {
    let step = match (key.code, key.modifiers) {
        (KeyCode::Right, KeyModifiers::NONE) => 1,
        (KeyCode::Left, KeyModifiers::NONE) => VARIANTS.len() - 1,
        _ => return false,
    };
    if !active() || !keys.is_pending() {
        return false;
    }
    VARIANT.store((current() + step) % VARIANTS.len(), Ordering::Relaxed);
    true
}

/// One row of the popup.
struct Entry {
    key: String,
    desc: String,
    icon: &'static str,
    colour: Color,
    section: &'static str,
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

/// The icon, its colour and the section for an intent (mini.icons style).
fn look(intent: &Intent) -> (&'static str, Color, &'static str) {
    match intent {
        Intent::NewSession => ("\u{f067}", GREEN, "session"),
        Intent::AddProject => ("\u{f07c}", BLUE, "session"),
        Intent::FilterProjects => ("\u{f0b0}", CYAN, "session"),
        Intent::PickModel => ("\u{f0e7}", MAGENTA, "session"),
        Intent::PickPermission => ("\u{f023}", YELLOW, "session"),
        Intent::ChangeWorkspace => ("\u{f1bb}", GREEN, "git"),
        Intent::SwitchBranch => ("\u{e725}", ORANGE, "git"),
        Intent::OpenTool(Tool::Shell) => ("\u{f120}", CYAN, "tools"),
        Intent::OpenTool(Tool::Lazygit) => ("\u{f1d3}", RED, "tools"),
        Intent::OpenTool(Tool::Nvim) => ("\u{e62b}", GREEN, "tools"),
        Intent::ToggleSidebar => ("\u{f0db}", BLUE1, "ui"),
        Intent::Top | Intent::SelectFirst => ("\u{f062}", BLUE, "motion"),
        Intent::ToggleFold => ("\u{f078}", BLUE, "fold"),
        _ => ("\u{f111}", DARK5, "other"),
    }
}

/// The keys under the pending sequence, in which-key's order: letters and
/// digits before symbols, lowercase before its capital.
fn entries(keys: &Keys) -> Vec<Entry> {
    let path = &keys.current_sequence;
    let scope = keys.scope();
    let children = keys
        .keymap()
        .children_at_path(path, scope)
        .unwrap_or_default();
    let mut entries: Vec<Entry> = children
        .into_iter()
        .map(|binding| {
            let full: Vec<KeyEvent> = path.iter().copied().chain([binding.key]).collect();
            match keys.keymap().navigate(&full, scope) {
                Some(NodeResult::Leaf { action }) => {
                    let (icon, colour, section) = look(&action);
                    Entry {
                        key: key_name(&binding.key),
                        desc: binding.description,
                        icon,
                        colour,
                        section,
                    }
                }
                _ => Entry {
                    key: key_name(&binding.key),
                    desc: format!("+{}", binding.description),
                    icon: "\u{f07b}",
                    colour: BLUE,
                    section: "groups",
                },
            }
        })
        .collect();
    entries.sort_by_key(|e| {
        let first = e.key.chars().next().unwrap_or(' ');
        (
            !first.is_alphanumeric(),
            e.key.to_lowercase(),
            first.is_uppercase(),
        )
    });
    entries
}

/// The pending path, e.g. `␣` or `g`.
fn path(keys: &Keys) -> String {
    keys.current_sequence
        .iter()
        .map(key_name)
        .collect::<Vec<_>>()
        .join(" ")
}

fn span<'a>(text: impl Into<std::borrow::Cow<'a, str>>, fg: Color) -> Span<'a> {
    Span::styled(text, Style::new().fg(fg))
}

/// `key ➜ icon desc`, the key right-aligned to `key_width`.
fn row(entry: &Entry, key_width: usize) -> Line<'_> {
    let desc = match entry.section {
        "groups" => BLUE,
        _ => MAGENTA,
    };
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

/// A floating box with `bg` behind it.
fn float(area: Rect, buf: &mut Buffer) {
    Clear.render(area, buf);
    buf.set_style(area, Style::new().bg(BG_DARK));
}

fn to_u16(n: usize) -> u16 {
    u16::try_from(n).unwrap_or(u16::MAX)
}

/// Draws the popup for the pending sequence over `area`, the screen above
/// the mode line.
pub(crate) fn render(keys: &Keys, area: Rect, buf: &mut Buffer) {
    if !keys.active && keys.current_sequence.is_empty() {
        return;
    }
    let entries = entries(keys);
    if entries.is_empty() || area.height < 4 {
        return;
    }
    match current() {
        1 => helix(keys, &entries, area, buf),
        2 => classic(keys, &entries, area, buf),
        3 => modern(keys, &entries, area, buf),
        _ => WhichKey::new().render(buf, keys),
    }
    label(area, buf);
}

/// The variant's name top-right, so a flip shows what changed.
fn label(area: Rect, buf: &mut Buffer) {
    let text = format!(" which-key: {}  ←/→ ", VARIANTS[current()]);
    let w = to_u16(width(&text));
    let x = area.right().saturating_sub(w + 1);
    buf.set_string(
        x,
        area.y,
        text,
        Style::new()
            .fg(BG_DARK)
            .bg(YELLOW)
            .add_modifier(Modifier::BOLD),
    );
}

fn helix(keys: &Keys, entries: &[Entry], area: Rect, buf: &mut Buffer) {
    let key_width = entries.iter().map(|e| width(&e.key)).max().unwrap_or(1);
    let inner_width = entries
        .iter()
        .map(|e| row_width(e, key_width))
        .max()
        .unwrap_or(0)
        .max(help().width());
    let width = to_u16(inner_width + 4).min(area.width);
    let height = to_u16(entries.len() + 3).min(area.height);
    let popup = Rect::new(
        area.right().saturating_sub(width + 1),
        area.bottom().saturating_sub(height),
        width,
        height,
    );
    float(popup, buf);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(BORDER))
        .title(Line::from(span(format!(" {} ", path(keys)), ORANGE)));
    let inner = block.inner(popup);
    block.render(popup, buf);
    let x = inner.x + 1;
    let rows = inner.height.saturating_sub(1);
    for (entry, y) in entries.iter().zip(inner.y..inner.y + rows) {
        buf.set_line(x, y, &row(entry, key_width), inner.width - 1);
    }
    buf.set_line(x, inner.bottom() - 1, &help(), inner.width - 1);
}

fn classic(keys: &Keys, entries: &[Entry], area: Rect, buf: &mut Buffer) {
    let key_width = entries.iter().map(|e| width(&e.key)).max().unwrap_or(1);
    let column = entries
        .iter()
        .map(|e| row_width(e, key_width))
        .max()
        .unwrap_or(0)
        + 4;
    let usable = usize::from(area.width.saturating_sub(4));
    let columns = (usable / column).clamp(1, entries.len());
    let lines = entries.len().div_ceil(columns);
    let height = to_u16(lines + 3).min(area.height);
    let panel = Rect::new(
        area.x,
        area.bottom().saturating_sub(height),
        area.width,
        height,
    );
    float(panel, buf);
    Block::new()
        .borders(Borders::TOP)
        .border_style(Style::new().fg(BORDER))
        .render(panel, buf);
    for (i, entry) in entries.iter().enumerate() {
        let x = panel.x + 2 + to_u16((i / lines) * column);
        let y = panel.y + 1 + to_u16(i % lines);
        if y < panel.bottom() - 1 {
            buf.set_line(x, y, &row(entry, key_width), to_u16(column));
        }
    }
    let foot = panel.bottom() - 1;
    buf.set_line(
        panel.x + 2,
        foot,
        &Line::from(vec![
            span(path(keys), ORANGE),
            span("  ", COMMENT),
            span(format!("{} keys", entries.len()), COMMENT),
        ]),
        panel.width / 2,
    );
    let help = help();
    let help_width = to_u16(help.width());
    buf.set_line(
        panel.right().saturating_sub(help_width + 2),
        foot,
        &help,
        help_width,
    );
}

fn modern(keys: &Keys, entries: &[Entry], area: Rect, buf: &mut Buffer) {
    let mut sections: Vec<(&str, Vec<&Entry>)> = Vec::new();
    for entry in entries {
        match sections.iter_mut().find(|(name, _)| *name == entry.section) {
            Some((_, list)) => list.push(entry),
            None => sections.push((entry.section, vec![entry])),
        }
    }
    const ORDER: [&str; 8] = [
        "groups", "session", "git", "tools", "ui", "motion", "fold", "other",
    ];
    sections.sort_by_key(|(name, _)| ORDER.iter().position(|o| o == name));
    let key_width = entries.iter().map(|e| width(&e.key)).max().unwrap_or(1);
    let widths: Vec<usize> = sections
        .iter()
        .map(|(name, list)| {
            list.iter()
                .map(|e| key_width + 2 + 2 + width(&e.desc))
                .max()
                .unwrap_or(0)
                .max(width(name))
        })
        .collect();
    let gap = 4;
    let content = widths.iter().sum::<usize>() + gap * widths.len().saturating_sub(1);
    let tallest = sections.iter().map(|(_, l)| l.len()).max().unwrap_or(0);
    let width = to_u16(content + 6).min(area.width * 9 / 10).max(30);
    let height = to_u16(tallest + 5).min(area.height);
    let popup = Rect::new(
        area.x + (area.width.saturating_sub(width)) / 2,
        area.bottom().saturating_sub(height + 1),
        width,
        height,
    );
    float(popup, buf);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(BORDER))
        .title(
            Line::from(vec![
                span(" ", ORANGE),
                span(path(keys), ORANGE),
                span(" ", ORANGE),
            ])
            .alignment(Alignment::Center),
        )
        .title_bottom(
            Line::from(vec![
                span(" esc ", CYAN),
                span("close · ", COMMENT),
                span("⌫ ", CYAN),
                span("back ", COMMENT),
            ])
            .alignment(Alignment::Center),
        );
    let inner = block.inner(popup);
    block.render(popup, buf);
    let mut x = inner.x + 2;
    for ((name, list), w) in sections.iter().zip(&widths) {
        let w = to_u16(*w);
        if x + w > inner.right() {
            break;
        }
        buf.set_string(
            x,
            inner.y + 1,
            name.to_uppercase(),
            Style::new().fg(BLUE).add_modifier(Modifier::BOLD),
        );
        for (entry, y) in list
            .iter()
            .zip(inner.y + 2..inner.bottom().saturating_sub(1))
        {
            let line = Line::from(vec![
                span(format!("{:>key_width$}", entry.key), ORANGE),
                span("  ", COMMENT),
                span(format!("{} ", entry.icon), entry.colour),
                span(entry.desc.as_str(), FG_DARK),
            ]);
            buf.set_line(x, y, &line, w);
        }
        x += w + to_u16(gap);
    }
}

/// Every variant for a few sequences, as ANSI truecolor text.
pub fn dump(state: &mut orb_domain::AppState, width: u16, height: u16) -> String {
    use orb_domain::Focus;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::keymap::{self, Scope, Selection};

    let cases: [(Focus, &str, &[KeyCode]); 3] = [
        (Focus::Sidebar, "sidebar, ␣", &[KeyCode::Char(' ')]),
        (Focus::Sidebar, "sidebar, g", &[KeyCode::Char('g')]),
        (Focus::Preview, "preview, z", &[KeyCode::Char('z')]),
    ];
    let mut out = String::new();
    for (focus, name, presses) in cases {
        state.focus = focus;
        for variant in 0..VARIANTS.len() {
            VARIANT.store(variant, Ordering::Relaxed);
            let mut keys = Keys::new(
                keymap::keymap(),
                Scope::new(focus, Selection::of(&state.sessions)),
            );
            for code in presses {
                keymap::press(&mut keys, KeyEvent::new(*code, KeyModifiers::NONE));
            }
            let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("backend");
            terminal
                .draw(|frame| {
                    crate::render::render(
                        frame,
                        state,
                        None,
                        None,
                        &keys,
                        std::time::SystemTime::now(),
                        &mut Default::default(),
                        &mut Default::default(),
                        &mut Default::default(),
                    );
                })
                .expect("draw");
            let _ = writeln!(out, "\x1b[0m==== {name} · {}", VARIANTS[variant]);
            ansi(terminal.backend().buffer(), &mut out);
        }
    }
    out
}

fn ansi(buf: &Buffer, out: &mut String) {
    let rgb = |c: Color| match c {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => (0x22, 0x24, 0x36),
    };
    for y in 0..buf.area.height {
        let mut skip = 0;
        for x in 0..buf.area.width {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            let cell = &buf[(x, y)];
            let (fr, fg, fb) = match cell.fg {
                Color::Reset => (0xc8, 0xd3, 0xf5),
                c => rgb(c),
            };
            let (br, bg, bb) = rgb(cell.bg);
            let _ = write!(
                out,
                "\x1b[38;2;{fr};{fg};{fb};48;2;{br};{bg};{bb}m{}",
                cell.symbol()
            );
            skip = width(cell.symbol()).saturating_sub(1);
        }
        out.push_str("\x1b[0m\n");
    }
}
