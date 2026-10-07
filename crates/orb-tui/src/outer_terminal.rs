//! The terminal orb runs in: the modes orb switches on for itself, and what it
//! passes through from the pane's child.
//!
//! orb asks for kitty key reports (so every key reaches the child
//! unambiguously), bracketed paste, and focus changes. The mouse is captured
//! from startup until orb exits, and the child's cursor shape and clipboard
//! copies are passed through to the outer terminal. Everything is switched back
//! off on exit and on panic.

use std::io::{self, Write};
use std::panic;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ratatui::crossterm::cursor::SetCursorStyle;
use ratatui::crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
    EnableFocusChange, EnableMouseCapture, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::execute;

/// Switches on the modes orb needs from the outer terminal.
///
/// # Errors
///
/// Returns an error if writing to the terminal fails.
pub(crate) fn enable<W>(out: &mut W) -> io::Result<()>
where
    W: Write,
{
    execute!(
        out,
        PushKeyboardEnhancementFlags(
            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
        ),
        EnableBracketedPaste,
        EnableFocusChange,
        EnableMouseCapture,
    )
}

/// Switches off everything orb changed in the outer terminal. Must run while
/// the alternate screen is still active: kitty key flags are kept per screen.
///
/// # Errors
///
/// Returns an error if writing to the terminal fails.
pub(crate) fn disable<W>(out: &mut W) -> io::Result<()>
where
    W: Write,
{
    execute!(
        out,
        DisableMouseCapture,
        SetCursorStyle::DefaultUserShape,
        DisableFocusChange,
        DisableBracketedPaste,
        PopKeyboardEnhancementFlags,
    )
}

/// Restores the outer terminal on panic. Install it after ratatui's own hook,
/// so this one runs first, before ratatui leaves the alternate screen.
pub(crate) fn install_panic_hook() {
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        let _ = disable(&mut io::stdout());
        previous(info);
    }));
}

/// Copies `text` to the system clipboard through the outer terminal (OSC 52).
///
/// # Errors
///
/// Returns an error if writing to the terminal fails.
pub(crate) fn copy_to_clipboard<W>(out: &mut W, text: &str) -> io::Result<()>
where
    W: Write,
{
    write!(out, "\x1b]52;c;{}\x07", STANDARD.encode(text))?;
    out.flush()
}

/// Sets the outer terminal's cursor shape.
///
/// # Errors
///
/// Returns an error if writing to the terminal fails.
pub(crate) fn set_cursor_style<W>(out: &mut W, style: SetCursorStyle) -> io::Result<()>
where
    W: Write,
{
    execute!(out, style)
}

#[cfg(test)]
mod tests {
    use super::{copy_to_clipboard, enable};

    #[rstest::rstest]
    fn clipboard_copy_writes_osc52_sequence() {
        // Given an empty output.
        let mut out = Vec::new();

        // When copying "hello".
        let _ = copy_to_clipboard(&mut out, "hello");

        // Then the output is an OSC 52 clipboard write of the base64 text.
        assert_eq!(
            out, b"\x1b]52;c;aGVsbG8=\x07",
            "copy should write OSC 52 with base64 text"
        );
    }

    #[rstest::rstest]
    fn enable_captures_the_mouse() {
        // Given an empty output.
        let mut out = Vec::new();

        // When switching on orb's terminal modes.
        let _ = enable(&mut out);

        // Then the output turns on SGR mouse reporting.
        assert!(
            String::from_utf8_lossy(&out).contains("\x1b[?1006h"),
            "enable should capture the mouse"
        );
    }

    #[rstest::rstest]
    fn enable_asks_for_disambiguated_keys() {
        // Given an empty output.
        let mut out = Vec::new();

        // When switching on orb's terminal modes.
        let _ = enable(&mut out);

        // Then it pushes kitty's disambiguate and alternate-key flags (1 | 4).
        assert!(
            String::from_utf8_lossy(&out).contains("\x1b[>5u"),
            "enable should push kitty flags 5, so Ctrl+[ and Super arrive distinct"
        );
    }
}
