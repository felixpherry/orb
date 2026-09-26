//! Claude transcripts — where a session's transcript lives and what to title it.
//!
//! Claude appends one JSON object per line to `<sessionId>.jsonl` under its
//! projects directory. orb reads only the lines added since its last look, so
//! each line is parsed once, and titles a thread by the latest title the user
//! gave with `/rename`, else Claude's latest generated title, else the first
//! real prompt. The same pass notes the git branch the latest prompt ran on.

use std::{
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use serde_json::Value;

/// Where Claude writes the transcript of `session_id` started in `cwd`.
///
/// Claude names the project directory after the cwd with every character
/// that isn't an ASCII letter or digit replaced by `-`.
pub fn transcript_path(claude_dir: &Path, cwd: &Path, session_id: &str) -> PathBuf {
    let escaped: String = cwd
        .to_string_lossy()
        .encode_utf16()
        .map(|unit| match u8::try_from(unit) {
            Ok(byte) if byte.is_ascii_alphanumeric() => char::from(byte),
            _ => '-',
        })
        .collect();
    claude_dir
        .join("projects")
        .join(escaped)
        .join(format!("{session_id}.jsonl"))
}

/// Finds the transcript of `session_id`, or `None` if Claude hasn't written one
/// yet (a session has none until its first prompt).
///
/// Looks at the expected path first, then in every project directory.
pub fn locate(claude_dir: &Path, cwd: &Path, session_id: &str) -> Option<PathBuf> {
    let expected = transcript_path(claude_dir, cwd, session_id);
    if expected.is_file() {
        return Some(expected);
    }
    let file_name = format!("{session_id}.jsonl");
    fs::read_dir(claude_dir.join("projects"))
        .ok()?
        .flatten()
        .map(|entry| entry.path().join(&file_name))
        .find(|path| path.is_file())
}

/// The complete lines appended to a transcript since an offset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewLines {
    /// The new complete lines (lossy UTF-8), each ending in `\n`.
    pub text: String,
    /// Bytes read so far; pass it to the next read.
    pub offset: u64,
    /// The file was shorter than the offset, so it was read from the start.
    pub restarted: bool,
}

/// Reads the complete lines added to the transcript since `offset`.
///
/// A trailing line without its newline is left for the next read. A file
/// shorter than `offset` was replaced, so it's read again from the start.
///
/// # Errors
///
/// Returns an error if the transcript can't be opened or read.
pub fn read_new_lines(path: &Path, offset: u64) -> io::Result<NewLines> {
    let mut file = File::open(path)?;
    let restarted = file.metadata()?.len() < offset;
    let start = if restarted { 0 } else { offset };
    #[expect(
        clippy::verbose_file_reads,
        reason = "reads from the saved offset; `fs::read` would re-read the whole transcript"
    )]
    let bytes = {
        let mut bytes = Vec::new();
        file.seek(SeekFrom::Start(start))?;
        file.read_to_end(&mut bytes)?;
        let complete = bytes
            .iter()
            .rposition(|&byte| byte == b'\n')
            .map_or(0, |newline| newline + 1);
        bytes.truncate(complete);
        bytes
    };
    Ok(NewLines {
        text: String::from_utf8_lossy(&bytes).into_owned(),
        offset: start + bytes.len() as u64,
        restarted,
    })
}

/// The titles and branch after a scan, and how far the transcript has been
/// read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TitleScan {
    /// Claude's latest generated title, else the first prompt.
    pub title: Option<String>,
    /// The latest title the user gave with `/rename`.
    pub custom_title: Option<String>,
    /// The git branch the latest user line named.
    pub branch: Option<String>,
    /// An `ai-title` line gave a title in these lines.
    pub ai_titled: bool,
    /// Bytes read so far; pass it to the next scan.
    pub offset: u64,
}

/// Reads the complete lines added to the transcript since `offset` and
/// updates `title`, `custom_title`, and `branch` from them, noting whether
/// Claude generated a title.
///
/// # Errors
///
/// Returns an error if the transcript can't be opened or read.
pub fn scan_title(
    path: &Path,
    offset: u64,
    title: Option<String>,
    custom_title: Option<String>,
    branch: Option<String>,
) -> io::Result<TitleScan> {
    let new = read_new_lines(path, offset)?;
    let scan = TitleScan {
        title,
        custom_title,
        branch,
        ai_titled: false,
        offset: new.offset,
    };
    Ok(new.text.lines().fold(scan, next_title))
}

/// The titles and branch after one transcript line.
fn next_title(scan: TitleScan, line: &str) -> TitleScan {
    if !line.contains(r#""type":"user""#)
        && !line.contains(r#""type":"ai-title""#)
        && !line.contains(r#""type":"custom-title""#)
    {
        return scan;
    }
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return scan;
    };
    match value.get("type").and_then(Value::as_str) {
        Some("ai-title") => match text_field(&value, "aiTitle") {
            Some(title) => TitleScan {
                title: Some(title),
                ai_titled: true,
                ..scan
            },
            None => scan,
        },
        Some("custom-title") => TitleScan {
            custom_title: text_field(&value, "customTitle").or(scan.custom_title),
            ..scan
        },
        Some("user") => TitleScan {
            title: scan.title.or_else(|| prompt(&value)),
            branch: text_field(&value, "gitBranch").or(scan.branch),
            ..scan
        },
        _ => scan,
    }
}

/// A line's string field, trimmed, or `None` if it's missing or blank.
fn text_field(line: &Value, key: &str) -> Option<String> {
    line.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

/// The first line of a prompt the user typed, or `None` for anything else
/// Claude records as a user line (skill bodies, commands, tool results,
/// interruptions).
fn prompt(line: &Value) -> Option<String> {
    if line.get("isMeta").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let text = match line.pointer("/message/content")? {
        Value::String(text) => text,
        Value::Array(blocks) => blocks
            .iter()
            .find(|block| block.get("type").and_then(Value::as_str) == Some("text"))?
            .get("text")?
            .as_str()?,
        _ => return None,
    }
    .trim();
    match text {
        "" => None,
        _ if text.starts_with('<') || text.starts_with("[Request interrupted") => None,
        _ => text.lines().next().map(|first| first.trim().to_owned()),
    }
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "tests propagate file failures with `?` and assert on the outcome"
)]
mod tests {
    use std::{
        fs::{self, OpenOptions},
        io::{self, Write},
        path::{Path, PathBuf},
    };

    use tempfile::tempdir;

    use super::{NewLines, TitleScan, locate, read_new_lines, scan_title, transcript_path};

    const PROMPT: &str = r#"{"type":"user","message":{"role":"user","content":"Fix the parser\nIt drops the last line"}}"#;

    fn write_transcript(dir: &Path, lines: &[&str]) -> io::Result<PathBuf> {
        let path = dir.join("session.jsonl");
        fs::write(&path, format!("{}\n", lines.join("\n")))?;
        Ok(path)
    }

    fn title_of(lines: &[&str]) -> io::Result<Option<String>> {
        let dir = tempdir()?;
        let path = write_transcript(dir.path(), lines)?;
        Ok(scan_title(&path, 0, None, None, None)?.title)
    }

    #[rstest::rstest]
    fn transcript_path_replaces_every_non_alphanumeric_character_with_a_dash() {
        // Given a cwd with slashes, a dot, and an underscore.
        let cwd = Path::new("/Users/me/.t3/a_b");

        // When computing its transcript path.
        let path = transcript_path(Path::new("/claude"), cwd, "abc");

        // Then each of those characters became a dash.
        assert_eq!(
            path,
            PathBuf::from("/claude/projects/-Users-me--t3-a-b/abc.jsonl"),
            "the project directory should be the escaped cwd"
        );
    }

    #[rstest::rstest]
    fn locate_finds_a_transcript_in_another_project_directory() -> io::Result<()> {
        // Given the transcript only exists under a different project directory.
        let claude_dir = tempdir()?;
        let other = claude_dir.path().join("projects").join("-somewhere-else");
        fs::create_dir_all(&other)?;
        fs::write(other.join("abc.jsonl"), "")?;

        // When locating it for a cwd whose escaped directory doesn't have it.
        let found = locate(claude_dir.path(), Path::new("/Users/me/orb"), "abc");

        // Then the scan of the project directories finds it.
        assert_eq!(
            found,
            Some(other.join("abc.jsonl")),
            "the scan should find the transcript"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn locate_returns_none_without_a_transcript() -> io::Result<()> {
        // Given a Claude directory with no transcript for the session.
        let claude_dir = tempdir()?;
        fs::create_dir_all(claude_dir.path().join("projects").join("-Users-me-orb"))?;

        // When locating it.
        let found = locate(claude_dir.path(), Path::new("/Users/me/orb"), "abc");

        // Then nothing is found.
        assert_eq!(found, None, "a prompt-less session has no transcript");
        Ok(())
    }

    #[rstest::rstest]
    fn first_prompt_titles_the_thread_by_its_first_line() -> io::Result<()> {
        // Given a transcript whose first prompt spans two lines.
        // When scanning it.
        let title = title_of(&[PROMPT])?;

        // Then the title is the prompt's first line.
        assert_eq!(
            title.as_deref(),
            Some("Fix the parser"),
            "title should be the first line"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn latest_ai_title_replaces_the_prompt_title() -> io::Result<()> {
        // Given a prompt followed by two generated titles.
        // When scanning it.
        let title = title_of(&[
            PROMPT,
            r#"{"type":"ai-title","aiTitle":"Parser fix"}"#,
            r#"{"type":"ai-title","aiTitle":"Fix last-line drop in parser"}"#,
        ])?;

        // Then the title is the latest generated one.
        assert_eq!(
            title.as_deref(),
            Some("Fix last-line drop in parser"),
            "the latest ai-title should win"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn ai_title_line_marks_the_scan_ai_titled() -> io::Result<()> {
        // Given a prompt followed by a generated title.
        let dir = tempdir()?;
        let path = write_transcript(
            dir.path(),
            &[PROMPT, r#"{"type":"ai-title","aiTitle":"Parser fix"}"#],
        )?;

        // When scanning it.
        let scan = scan_title(&path, 0, None, None, None)?;

        // Then the scan notes that Claude titled the thread.
        assert!(scan.ai_titled, "an ai-title line should mark the scan");
        Ok(())
    }

    #[rstest::rstest]
    fn custom_title_after_an_ai_title_is_kept_beside_it() -> io::Result<()> {
        // Given a prompt, a generated title, then a `/rename`.
        let dir = tempdir()?;
        let path = write_transcript(
            dir.path(),
            &[
                PROMPT,
                r#"{"type":"ai-title","aiTitle":"Parser fix"}"#,
                r#"{"type":"custom-title","customTitle":" orb-m1 ","sessionId":"s1"}"#,
            ],
        )?;

        // When scanning it.
        let scan = scan_title(&path, 0, None, None, None)?;

        // Then the custom title is set and the title is still the generated one.
        assert_eq!(
            (scan.title.as_deref(), scan.custom_title.as_deref()),
            (Some("Parser fix"), Some("orb-m1")),
            "`/rename` should be kept apart from the ai-title"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn later_prompt_keeps_the_existing_title() -> io::Result<()> {
        // Given a titled thread whose transcript gains another prompt.
        let dir = tempdir()?;
        let path = write_transcript(
            dir.path(),
            &[r#"{"type":"user","message":{"content":"Now add tests"}}"#],
        )?;

        // When scanning the new prompt.
        let scan = scan_title(&path, 0, Some("Fix the parser".to_owned()), None, None)?;

        // Then the title is unchanged.
        assert_eq!(
            scan.title.as_deref(),
            Some("Fix the parser"),
            "later prompts don't retitle"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn scan_takes_the_latest_git_branch() -> io::Result<()> {
        // Given two prompts that ran on different branches.
        let dir = tempdir()?;
        let path = write_transcript(
            dir.path(),
            &[
                r#"{"type":"user","gitBranch":"main","message":{"content":"Fix the parser"}}"#,
                r#"{"type":"user","gitBranch":"fix/parser","message":{"content":"Now add tests"}}"#,
            ],
        )?;

        // When scanning them.
        let scan = scan_title(&path, 0, None, None, None)?;

        // Then the branch is the one the latest prompt named.
        assert_eq!(
            scan.branch.as_deref(),
            Some("fix/parser"),
            "the latest gitBranch should win"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn scan_ignores_an_empty_git_branch() -> io::Result<()> {
        // Given a thread on `main` whose transcript gains a prompt with no branch.
        let dir = tempdir()?;
        let path = write_transcript(
            dir.path(),
            &[r#"{"type":"user","gitBranch":"","message":{"content":"Now add tests"}}"#],
        )?;

        // When scanning the new prompt.
        let scan = scan_title(&path, 0, None, None, Some("main".to_owned()))?;

        // Then the branch is still `main`.
        assert_eq!(
            scan.branch.as_deref(),
            Some("main"),
            "an empty gitBranch means no branch, not a change"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[case::meta(
        r#"{"type":"user","isMeta":true,"message":{"content":"Base directory for this skill"}}"#
    )]
    #[case::command_name(
        r#"{"type":"user","message":{"content":"<command-name>/plan</command-name>"}}"#
    )]
    #[case::local_command_caveat(r#"{"type":"user","message":{"content":"<local-command-caveat>Caveat: do not respond</local-command-caveat>"}}"#)]
    #[case::tool_result(r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}}"#)]
    #[case::interrupted(r#"{"type":"user","message":{"content":"[Request interrupted by user]"}}"#)]
    fn non_prompt_user_lines_are_skipped_for_the_title(#[case] skipped: &str) -> io::Result<()> {
        // Given a user line that isn't a typed prompt, then a real prompt.
        // When scanning them.
        let title = title_of(&[skipped, PROMPT])?;

        // Then the title comes from the real prompt.
        assert_eq!(
            title.as_deref(),
            Some("Fix the parser"),
            "non-prompt lines should be skipped"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn text_block_of_a_prompt_array_titles_the_thread() -> io::Result<()> {
        // Given a prompt made of an image block and a text block.
        // When scanning it.
        let title = title_of(&[
            r#"{"type":"user","message":{"content":[{"type":"image","source":{"type":"base64","data":"AA=="}},{"type":"text","text":"What's in this screenshot?"}]}}"#,
        ])?;

        // Then the title is the text block's text.
        assert_eq!(
            title.as_deref(),
            Some("What's in this screenshot?"),
            "title should come from the text block"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn scan_leaves_a_partial_last_line_unread() -> io::Result<()> {
        // Given a complete prompt line followed by a line still being written.
        let dir = tempdir()?;
        let path = dir.path().join("session.jsonl");
        fs::write(&path, format!("{PROMPT}\n{{\"type\":\"ai-title\",\"aiT"))?;

        // When scanning it.
        let scan = scan_title(&path, 0, None, None, None)?;

        // Then only the complete line was read.
        assert_eq!(
            scan,
            TitleScan {
                title: Some("Fix the parser".to_owned()),
                custom_title: None,
                branch: None,
                ai_titled: false,
                offset: PROMPT.len() as u64 + 1
            },
            "the offset should stop after the last newline"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn completed_partial_line_is_parsed_on_the_next_scan() -> io::Result<()> {
        // Given a scan that stopped before a partial line, which is then completed.
        let dir = tempdir()?;
        let path = dir.path().join("session.jsonl");
        fs::write(&path, format!("{PROMPT}\n{{\"type\":\"ai-title\",\"aiT"))?;
        let first = scan_title(&path, 0, None, None, None)?;
        OpenOptions::new()
            .append(true)
            .open(&path)?
            .write_all(b"itle\":\"Parser fix\"}\n")?;

        // When scanning again from the first scan's offset.
        let scan = scan_title(&path, first.offset, first.title, None, None)?;

        // Then the completed line applied and the offset reached the file end.
        assert_eq!(
            scan,
            TitleScan {
                title: Some("Parser fix".to_owned()),
                custom_title: None,
                branch: None,
                ai_titled: true,
                offset: fs::metadata(&path)?.len()
            },
            "the completed line should be parsed once"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn malformed_line_is_skipped() -> io::Result<()> {
        // Given a truncated user line followed by a real prompt.
        // When scanning them.
        let title = title_of(&[r#"{"type":"user","message":{"content":"Bro"#, PROMPT])?;

        // Then the malformed line is ignored and the prompt still titles the thread.
        assert_eq!(
            title.as_deref(),
            Some("Fix the parser"),
            "malformed lines should be skipped"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn file_shorter_than_the_offset_is_rescanned_from_the_start() -> io::Result<()> {
        // Given a transcript shorter than the saved offset (the file was replaced).
        let dir = tempdir()?;
        let path = write_transcript(dir.path(), &[PROMPT])?;

        // When scanning from that offset.
        let scan = scan_title(&path, 10_000, None, None, None)?;

        // Then the whole file was read again.
        assert_eq!(
            scan,
            TitleScan {
                title: Some("Fix the parser".to_owned()),
                custom_title: None,
                branch: None,
                ai_titled: false,
                offset: fs::metadata(&path)?.len()
            },
            "a replaced file should be read from the start"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn read_new_lines_restarts_on_a_file_shorter_than_the_offset() -> io::Result<()> {
        // Given a transcript shorter than the saved offset (the file was replaced).
        let dir = tempdir()?;
        let path = write_transcript(dir.path(), &[PROMPT])?;

        // When reading from that offset.
        let new = read_new_lines(&path, 10_000)?;

        // Then it restarted and read the whole file.
        assert_eq!(
            new,
            NewLines {
                text: format!("{PROMPT}\n"),
                offset: fs::metadata(&path)?.len(),
                restarted: true,
            },
            "a replaced file should be read from the start"
        );
        Ok(())
    }
}
