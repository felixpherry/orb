//! The environment every process orb starts runs with: the terminal pane's
//! child and every `claude` call.
//!
//! The child inherits orb's environment minus two groups of variables: the ones
//! that would make it think it runs inside a Claude session, and the ones that
//! name the terminal orb itself runs in. In their place it gets one fixed
//! terminal identity, so Claude picks the same capabilities (kitty keys,
//! synchronized output) no matter how orb was launched.

use std::ffi::OsString;

/// The terminal identity every child gets.
const IDENTITY: [(&str, &str); 4] = [
    ("TERM", "xterm-256color"),
    ("COLORTERM", "truecolor"),
    ("TERM_PROGRAM", "WezTerm"),
    ("CLAUDE_CODE_FORCE_SYNC_OUTPUT", "1"),
];

/// Variables a Claude session sets for the processes it spawns.
const CLAUDE_SESSION_VARS: [&str; 12] = [
    "CLAUDECODE",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_PID",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_SESSION_KIND",
    "CLAUDE_EFFORT",
    "CLAUDE_AGENT_SDK_VERSION",
];

/// Variables that identify the outer terminal or force its colours.
const TERMINAL_VARS: [&str; 11] = [
    "STY",
    "WINDOWID",
    "TERMINATOR_UUID",
    "TILIX_ID",
    "VTE_VERSION",
    "WT_SESSION",
    "TERMINAL_EMULATOR",
    "ZED_TERM",
    "COLORFGBG",
    "__CFBundleIdentifier",
    "FORCE_COLOR",
];

/// Prefixes of variables that identify the outer terminal or multiplexer.
const TERMINAL_PREFIXES: [&str; 10] = [
    "KITTY_",
    "ZELLIJ",
    "TMUX",
    "TERM_PROGRAM",
    "LC_TERMINAL",
    "ITERM_",
    "WEZTERM_",
    "GHOSTTY_",
    "ALACRITTY_",
    "KONSOLE_",
];

/// The child's environment: `parent` without Claude session and outer-terminal
/// variables, plus orb's terminal identity.
pub fn child_env<I>(parent: I) -> Vec<(OsString, OsString)>
where
    I: IntoIterator<Item = (OsString, OsString)>,
{
    parent
        .into_iter()
        .filter(|(key, _)| !key.to_str().is_some_and(is_scrubbed))
        .chain(IDENTITY.map(|(key, value)| (key.into(), value.into())))
        .collect()
}

fn is_scrubbed(key: &str) -> bool {
    CLAUDE_SESSION_VARS.contains(&key)
        || TERMINAL_VARS.contains(&key)
        || IDENTITY.iter().any(|(name, _)| *name == key)
        || TERMINAL_PREFIXES
            .iter()
            .any(|prefix| key.starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::child_env;

    fn env_of(vars: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        child_env(
            vars.iter()
                .map(|(key, value)| (OsString::from(key), OsString::from(value))),
        )
    }

    fn values_of<'a>(env: &'a [(OsString, OsString)], key: &str) -> Vec<&'a OsString> {
        env.iter()
            .filter(|(name, _)| name == key)
            .map(|(_, value)| value)
            .collect()
    }

    #[rstest::rstest]
    #[case("CLAUDECODE")]
    #[case("CLAUDE_CODE_CHILD_SESSION")]
    #[case("CLAUDE_CODE_SESSION_ID")]
    #[case("CLAUDE_CODE_MESSAGING_SOCKET")]
    #[case("CLAUDE_CODE_MESSAGING_TOKEN")]
    #[case("CLAUDE_CODE_ENTRYPOINT")]
    #[case("CLAUDE_PID")]
    #[case("CLAUDE_CODE_EXECPATH")]
    #[case("CLAUDE_CODE_SESSION_ATTENDED")]
    #[case("CLAUDE_CODE_SESSION_KIND")]
    #[case("CLAUDE_EFFORT")]
    #[case("CLAUDE_AGENT_SDK_VERSION")]
    fn claude_session_variables_are_removed(#[case] key: &str) {
        // Given orb runs inside a Claude session.
        let parent = [(key, "1")];

        // When building the child's environment.
        let env = env_of(&parent);

        // Then the session variable is gone.
        assert!(values_of(&env, key).is_empty(), "{key} should be removed");
    }

    #[rstest::rstest]
    #[case("KITTY_WINDOW_ID")]
    #[case("KITTY_PID")]
    #[case("ZELLIJ")]
    #[case("ZELLIJ_SESSION_NAME")]
    #[case("TMUX")]
    #[case("TERM_PROGRAM_VERSION")]
    #[case("LC_TERMINAL")]
    #[case("ITERM_SESSION_ID")]
    #[case("WT_SESSION")]
    #[case("VTE_VERSION")]
    #[case("__CFBundleIdentifier")]
    #[case("FORCE_COLOR")]
    fn outer_terminal_identity_is_removed(#[case] key: &str) {
        // Given orb runs in a terminal that identifies itself.
        let parent = [(key, "1")];

        // When building the child's environment.
        let env = env_of(&parent);

        // Then the identity variable is gone.
        assert!(values_of(&env, key).is_empty(), "{key} should be removed");
    }

    #[rstest::rstest]
    #[case("PATH", "/usr/bin:/bin")]
    #[case("HOME", "/Users/me")]
    #[case("CLAUDE_CONFIG_DIR", "/Users/me/.claude-work")]
    #[case("LANG", "en_US.UTF-8")]
    fn user_variables_are_kept(#[case] key: &str, #[case] value: &str) {
        // Given a user variable in orb's environment.
        let parent = [(key, value)];

        // When building the child's environment.
        let env = env_of(&parent);

        // Then the child sees it unchanged.
        assert_eq!(
            values_of(&env, key),
            [&OsString::from(value)],
            "{key} should be kept as is"
        );
    }

    #[rstest::rstest]
    #[case("TERM", "xterm-256color")]
    #[case("COLORTERM", "truecolor")]
    #[case("TERM_PROGRAM", "WezTerm")]
    #[case("CLAUDE_CODE_FORCE_SYNC_OUTPUT", "1")]
    fn terminal_identity_is_set(#[case] key: &str, #[case] value: &str) {
        // Given orb runs in kitty.
        let parent = [("TERM", "xterm-kitty"), ("TERM_PROGRAM", "kitty")];

        // When building the child's environment.
        let env = env_of(&parent);

        // Then the child has exactly one of orb's identity values.
        assert_eq!(
            values_of(&env, key),
            [&OsString::from(value)],
            "{key} should be set once to {value}"
        );
    }
}
