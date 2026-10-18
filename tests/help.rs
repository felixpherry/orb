//! `orb help`, `--help` and a mistyped first word, run as a user runs them.
#![expect(
    clippy::panic_in_result_fn,
    reason = "tests propagate setup failures with `?` and assert on the outcome"
)]

use std::io;
use std::path::Path;
use std::process::{Command, Output};
use std::time::Duration;

use orb_domain::{Finished, run_within};

/// Runs the built orb with `words`, `HOME` at `home` and outside any orb
/// pane, so nothing it does can reach a real orb; `None` when it ran past
/// 5 s.
fn orb(words: &[&str], home: &Path) -> io::Result<Option<Output>> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_orb"));
    command
        .args(words)
        .env("HOME", home)
        .env_remove("ORB_PANE_ID");
    let finished = run_within(command, Duration::from_secs(5))?;
    Ok(match finished {
        Finished::Exited(output) => Some(output),
        Finished::TimedOut => None,
    })
}

#[rstest::rstest]
#[case::help_word(&["help"], "orb: drive the running orb")]
#[case::help_flag(&["--help"], "orb: drive the running orb")]
#[case::short_help(&["-h"], "orb: drive the running orb")]
#[case::noun_help(&["pane", "--help"], "orb pane: ")]
#[case::project_help(&["project", "--help"], "orb project: ")]
#[case::command_help(&["pane", "split", "--help"], "usage: orb pane split")]
fn help_words_print_help_and_exit_0(
    #[case] words: &[&str],
    #[case] expected: &str,
) -> io::Result<()> {
    // Given the built orb with a fresh home.
    let home = tempfile::tempdir()?;

    // When running it with the help words.
    let output = orb(words, home.path())?;

    // Then it printed the help and exited 0.
    let printed = output
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned());
    assert!(
        printed
            .as_deref()
            .is_some_and(|printed| printed.contains(expected)),
        "{words:?} should print help holding {expected:?} and exit 0, got {printed:?}"
    );
    Ok(())
}

#[rstest::rstest]
fn unknown_first_word_is_a_usage_error_and_starts_nothing() -> io::Result<()> {
    // Given the built orb with a fresh home.
    let home = tempfile::tempdir()?;

    // When running `orb sessions`.
    let output = orb(&["sessions"], home.path())?;

    // Then it exited 2 at once, naming the word and `orb help`, without
    // trying to start orb.
    let ended = output.map(|output| {
        (
            output.status.code(),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    });
    assert!(
        ended.as_ref().is_some_and(|(code, out, err)| {
            *code == Some(2)
                && out.is_empty()
                && err.contains("sessions")
                && err.contains("help: orb help")
                && !err.contains("already running")
        }),
        "an unknown first word is a usage error, got {ended:?}"
    );
    Ok(())
}
