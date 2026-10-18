//! orb's integrations with Claude Code and pi: a Claude hook and a pi
//! extension that report, through a file per pane under `~/.orb/panes/`,
//! which conversation runs in which pane, and orb's skills in Claude's and
//! pi's skills folders: the `orb` skill that teaches agents to drive orb, and
//! the workflow skills that take an approved plan to an open pull request.
//! `orb integration install` puts them in place.

pub mod claude_settings;
pub mod pane_file;
mod skills;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use error_stack::{Report, ResultExt};
use wherror::Error;

use crate::tilde;

/// The hook script's file name; orb's entries in Claude's settings are found
/// by it.
pub const SCRIPT_NAME: &str = "orb-agent-state.sh";
const EXTENSION_NAME: &str = "orb-agent-state.ts";
const SCRIPT: &str = include_str!("assets/orb-agent-state.sh");
const EXTENSION: &str = include_str!("assets/orb-agent-state.ts");
/// Where older orb versions put pi's copy of the `orb` skill, under pi's
/// agent dir.
const OLD_PI_SKILL: &str = "skills/orb";
/// What the mode line shows at start when neither integration is installed.
pub const NUDGE: &str = "run orb integration install";

/// Installing an integration failed. The report names the file.
#[derive(Debug, Error)]
#[error(debug)]
pub struct IntegrationError;

/// Where each integration lives.
#[derive(Debug, Clone)]
pub struct IntegrationPaths {
    /// orb's folder (`~/.orb`): the hook script and the pane files.
    pub orb_root: PathBuf,
    /// Claude's config home: `$CLAUDE_CONFIG_DIR`, else `~/.claude`.
    pub claude_dir: PathBuf,
    /// pi's agent dir: `$PI_CODING_AGENT_DIR`, else `~/.pi/agent`.
    pub pi_dir: PathBuf,
    /// The shared skills folder pi reads (`~/.agents/skills`).
    pub agents_skills_dir: PathBuf,
}

/// Claude's config home: `config_dir` (`$CLAUDE_CONFIG_DIR`) when set, else
/// `~/.claude`.
pub fn claude_dir(config_dir: Option<PathBuf>, home: &Path) -> PathBuf {
    config_dir.unwrap_or_else(|| home.join(".claude"))
}

/// pi's agent dir: `agent_dir` (`$PI_CODING_AGENT_DIR`) when set, else
/// `~/.pi/agent`.
pub fn pi_agent_dir(agent_dir: Option<PathBuf>, home: &Path) -> PathBuf {
    agent_dir.unwrap_or_else(|| home.join(".pi/agent"))
}

/// The folder the agents write their pane files in.
pub fn panes_dir(orb_root: &Path) -> PathBuf {
    orb_root.join("panes")
}

impl IntegrationPaths {
    pub fn hook_script(&self) -> PathBuf {
        self.orb_root.join("hooks").join(SCRIPT_NAME)
    }

    pub fn claude_settings(&self) -> PathBuf {
        self.claude_dir.join("settings.json")
    }

    pub fn pi_extension(&self) -> PathBuf {
        self.pi_dir.join("extensions").join(EXTENSION_NAME)
    }

    /// Whether either integration has been installed.
    pub fn any_installed(&self) -> bool {
        self.hook_script().exists() || self.pi_extension().exists()
    }
}

/// Writes the hook script, adds orb's hooks to Claude's `settings.json`,
/// writes the pi extension, writes orb's skills into Claude's skills folder
/// and into `~/.agents/skills` for pi, and removes the `orb` skill copy older
/// versions wrote under pi's agent dir unless that copy is one of the folders
/// written. Returns one line per step for the user, `~` for `home`.
///
/// # Errors
///
/// Returns an error naming the file or folder if one can't be written or
/// removed, or if `settings.json` isn't a JSON object (comments included);
/// files written before the failure stay.
pub fn install(
    paths: &IntegrationPaths,
    home: &Path,
) -> Result<Vec<String>, Report<IntegrationError>> {
    let script = paths.hook_script();
    write_atomic(&script, SCRIPT)?;
    let settings = paths.claude_settings();
    let command = claude_settings::hook_command(&script);
    let before = match fs::read_to_string(&settings) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(Report::new(error)
                .change_context(IntegrationError)
                .attach(format!("couldn't read {}", settings.display())));
        }
    };
    let after = claude_settings::merge(before.as_deref(), &command)
        .attach_with(|| format!("{} wasn't changed", settings.display()))?;
    let settings_line = if before.as_deref() == Some(after.as_str()) {
        format!("{} already has orb's hooks", tilde(&settings, home))
    } else {
        // ponytail: Claude may rewrite settings.json itself (`/model` and
        // others) between this read and write, and that edit would be lost;
        // re-read and retry when the file changed under the write if a lost
        // edit is ever seen.
        // A symlinked settings file is replaced at its target, keeping the link.
        let target = fs::canonicalize(&settings).unwrap_or_else(|_| settings.clone());
        write_atomic(&target, &after)?;
        format!(
            "added SessionStart and SessionEnd hooks to {}",
            tilde(&settings, home)
        )
    };
    let extension = paths.pi_extension();
    write_atomic(&extension, EXTENSION)?;
    let mut lines = vec![
        format!("wrote {}", tilde(&script, home)),
        settings_line,
        format!("wrote {}", tilde(&extension, home)),
    ];
    let claude_skills = paths.claude_dir.join("skills");
    let skill_dirs = [&claude_skills, &paths.agents_skills_dir];
    let old = paths.pi_dir.join(OLD_PI_SKILL);
    // When pi's skills folder links to a folder written below, the old copy is
    // the live orb skill, so it stays and the writes update it in place.
    let old_is_live = fs::canonicalize(&old).is_ok_and(|old| {
        skill_dirs
            .iter()
            .any(|dir| fs::canonicalize(dir.join("orb")).is_ok_and(|live| live == old))
    });
    if !old_is_live {
        match fs::remove_dir_all(&old) {
            Ok(()) => lines.push(format!("removed {}", tilde(&old, home))),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(Report::new(error)
                    .change_context(IntegrationError)
                    .attach(format!("couldn't remove {}", old.display())));
            }
        }
    }
    let count = skills::skill_count();
    for dir in skill_dirs {
        for (file, text) in skills::SKILLS {
            write_atomic(&dir.join(file), text)?;
        }
        lines.push(format!("wrote {count} skills to {}", tilde(dir, home)));
    }
    Ok(lines)
}

/// Writes `text` to `path` through a temp file beside it and a rename, making
/// the folder first and keeping the mode of a file it replaces.
fn write_atomic(path: &Path, text: &str) -> Result<(), Report<IntegrationError>> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .change_context(IntegrationError)
            .attach_with(|| format!("couldn't create {}", parent.display()))?;
    }
    let temp = {
        let mut name = path.as_os_str().to_owned();
        name.push(".orb-tmp");
        PathBuf::from(name)
    };
    fs::write(&temp, text)
        .change_context(IntegrationError)
        .attach_with(|| format!("couldn't write {}", temp.display()))?;
    // A replaced file keeps its mode (settings.json may be private).
    if let Ok(meta) = fs::metadata(path) {
        fs::set_permissions(&temp, meta.permissions())
            .change_context(IntegrationError)
            .attach_with(|| format!("couldn't set the mode of {}", temp.display()))?;
    }
    fs::rename(&temp, path)
        .change_context(IntegrationError)
        .attach_with(|| format!("couldn't replace {}", path.display()))
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "tests set files up with `?` and assert on the outcome"
)]
mod tests {
    use std::fs;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::{Command, ExitStatus, Stdio};
    use std::time::Duration;

    use error_stack::{Report, ResultExt};
    use serde_json::{Value, json};
    use tempfile::TempDir;

    use super::skills::{LOOP_RULES, SKILLS};
    use super::{
        IntegrationError, IntegrationPaths, SCRIPT, claude_dir, claude_settings, install,
        pi_agent_dir,
    };
    use crate::common::wait_within;

    type TestResult = Result<(), Report<IntegrationError>>;

    /// How long the hook script may run before it's stopped.
    const HOOK_LIMIT: Duration = Duration::from_secs(5);

    /// A scratch home with orb's, Claude's and pi's folders inside it.
    struct Home {
        tmp: TempDir,
        paths: IntegrationPaths,
    }

    impl Home {
        fn new() -> Result<Self, Report<IntegrationError>> {
            let tmp = TempDir::new().change_context(IntegrationError)?;
            let paths = IntegrationPaths {
                orb_root: tmp.path().join(".orb"),
                claude_dir: tmp.path().join(".claude"),
                pi_dir: tmp.path().join(".pi/agent"),
                agents_skills_dir: tmp.path().join(".agents/skills"),
            };
            Ok(Self { tmp, paths })
        }

        fn path(&self) -> &Path {
            self.tmp.path()
        }

        fn install(&self) -> Result<Vec<String>, Report<IntegrationError>> {
            install(&self.paths, self.path())
        }
    }

    fn read(path: &Path) -> Result<String, Report<IntegrationError>> {
        fs::read_to_string(path).change_context(IntegrationError)
    }

    /// Runs the hook script in a clean environment with `HOME` = `home`,
    /// `vars` set and `input` on stdin.
    fn run_hook(home: &Path, vars: &[(&str, &str)], input: &Value) -> TestResult {
        hook_status(home, vars, &input.to_string()).map(drop)
    }

    /// Runs the hook script like [`run_hook`] with raw `input`, and returns
    /// how it exited.
    fn hook_status(
        home: &Path,
        vars: &[(&str, &str)],
        input: &str,
    ) -> Result<ExitStatus, Report<IntegrationError>> {
        let script = home.join("orb-agent-state.sh");
        fs::write(&script, SCRIPT).change_context(IntegrationError)?;
        let mut child = Command::new("sh")
            .arg(&script)
            .env_clear()
            .env("HOME", home)
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .envs(vars.iter().copied())
            .stdin(Stdio::piped())
            .spawn()
            .change_context(IntegrationError)?;
        if let Some(mut stdin) = child.stdin.take() {
            // A hook may exit without reading its input, closing the pipe first.
            match stdin.write_all(input.as_bytes()) {
                Err(error) if error.kind() != std::io::ErrorKind::BrokenPipe => {
                    return Err(Report::new(error).change_context(IntegrationError));
                }
                _ => {}
            }
        }
        wait_within(&mut child, HOOK_LIMIT)
            .change_context(IntegrationError)?
            .ok_or_else(|| Report::new(IntegrationError).attach("the hook ran past its limit"))
    }

    fn pane_file(home: &Path) -> PathBuf {
        home.join(".orb/panes/7.json")
    }

    fn start_input() -> Value {
        json!({
            "hook_event_name": "SessionStart",
            "session_id": "s-1",
            "transcript_path": "/t/s-1.jsonl",
            "cwd": "/w",
            "source": "startup",
        })
    }

    fn end_input(reason: &str) -> Value {
        json!({
            "hook_event_name": "SessionEnd",
            "session_id": "s-1",
            "transcript_path": "/t/s-1.jsonl",
            "reason": reason,
        })
    }

    /// `input` with `key` set to `value`.
    fn with(mut input: Value, key: &str, value: &str) -> Value {
        if let Some(object) = input.as_object_mut() {
            object.insert(key.to_owned(), json!(value));
        }
        input
    }

    #[rstest::rstest]
    fn hook_start_writes_the_session_and_transcript_to_the_pane_file() -> TestResult {
        // Given a Claude session starting in pane 7.
        let home = Home::new()?;

        // When the hook runs on SessionStart.
        run_hook(home.path(), &[("ORB_PANE_ID", "7")], &start_input())?;

        // Then pane 7's file names the session, its transcript and its source.
        let report: Value = serde_json::from_str(&read(&pane_file(home.path()))?)
            .change_context(IntegrationError)?;
        let found = (
            report.get("agent").cloned(),
            report.get("event").cloned(),
            report.get("session_id").cloned(),
            report.get("transcript").cloned(),
            report.get("source").cloned(),
        );
        let expected = (
            Some(json!("claude")),
            Some(json!("start")),
            Some(json!("s-1")),
            Some(json!("/t/s-1.jsonl")),
            Some(json!("startup")),
        );
        assert_eq!(found, expected, "the pane file should hold the start");
        Ok(())
    }

    #[rstest::rstest]
    fn hook_end_writes_an_end() -> TestResult {
        // Given a Claude session in pane 7.
        let home = Home::new()?;

        // When the hook runs on SessionEnd because the user quit.
        run_hook(
            home.path(),
            &[("ORB_PANE_ID", "7")],
            &end_input("prompt_input_exit"),
        )?;

        // Then pane 7's file holds an end.
        let report: Value = serde_json::from_str(&read(&pane_file(home.path()))?)
            .change_context(IntegrationError)?;
        assert_eq!(
            report.get("event"),
            Some(&json!("end")),
            "the pane file should hold the end"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[case::outside_a_pane(&[], start_input())]
    #[case::subagent(&[("ORB_PANE_ID", "7")], with(start_input(), "agent_id", "a-1"))]
    #[case::cursor_input(&[("ORB_PANE_ID", "7")], with(start_input(), "cursor_version", "1.0"))]
    #[case::cursor_env(&[("ORB_PANE_ID", "7"), ("CURSOR_VERSION", "1.0")], start_input())]
    #[case::end_before_clear(&[("ORB_PANE_ID", "7")], end_input("clear"))]
    #[case::end_before_resume(&[("ORB_PANE_ID", "7")], end_input("resume"))]
    #[case::odd_pane_id(&[("ORB_PANE_ID", "../x")], start_input())]
    fn hook_writes_nothing(#[case] vars: &[(&str, &str)], #[case] input: Value) -> TestResult {
        // Given a hook call orb shouldn't record.
        let home = Home::new()?;

        // When the hook runs.
        run_hook(home.path(), vars, &input)?;

        // Then no pane file is written.
        let panes = home.path().join(".orb/panes");
        let written = fs::read_dir(&panes).map_or(0, Iterator::count);
        assert_eq!(written, 0, "no pane file should be written");
        Ok(())
    }

    #[rstest::rstest]
    fn hook_exits_zero_on_input_that_isnt_json() -> TestResult {
        // Given a hook call in pane 7 whose input is garbage.
        let home = Home::new()?;

        // When the hook runs.
        let status = hook_status(home.path(), &[("ORB_PANE_ID", "7")], "not json")?;

        // Then it still succeeds, so Claude carries on.
        assert!(status.success(), "the hook should always exit 0");
        Ok(())
    }

    #[rstest::rstest]
    fn install_writes_the_hook_script() -> TestResult {
        // Given an empty home.
        let home = Home::new()?;

        // When installing.
        home.install()?;

        // Then orb's hook script is in ~/.orb/hooks.
        let script = read(&home.paths.hook_script())?;
        assert_eq!(script, SCRIPT, "the hook script should be written");
        Ok(())
    }

    #[rstest::rstest]
    fn install_writes_the_pi_extension_in_the_pi_agent_dir() -> TestResult {
        // Given an empty home.
        let home = Home::new()?;

        // When installing.
        home.install()?;

        // Then the extension is in pi's extensions folder.
        let extension = home.path().join(".pi/agent/extensions/orb-agent-state.ts");
        assert!(extension.is_file(), "the pi extension should be written");
        Ok(())
    }

    /// The embedded contents of the skill file at `file`.
    fn skill(file: &str) -> Option<&'static str> {
        SKILLS
            .iter()
            .find(|(path, _)| *path == file)
            .map(|(_, text)| *text)
    }

    /// Every skill file under `dir`, keyed by its path inside `dir`.
    fn skill_files(dir: &Path) -> Result<Vec<(String, String)>, Report<IntegrationError>> {
        SKILLS
            .iter()
            .map(|(file, _)| Ok(((*file).to_owned(), read(&dir.join(file))?)))
            .collect()
    }

    #[rstest::rstest]
    #[case::claude(".claude/skills/orb/SKILL.md")]
    #[case::pi(".agents/skills/orb/SKILL.md")]
    fn install_writes_the_orb_skill(#[case] file: &str) -> TestResult {
        // Given an empty home.
        let home = Home::new()?;

        // When installing.
        home.install()?;

        // Then the agent's skills folder holds orb's skill.
        assert_eq!(
            Some(read(&home.path().join(file))?.as_str()),
            skill("orb/SKILL.md"),
            "the orb skill should be written"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[case::claude(".claude/skills")]
    #[case::pi(".agents/skills")]
    fn install_writes_every_skill_file(#[case] dir: &str) -> TestResult {
        // Given an empty home.
        let home = Home::new()?;

        // When installing.
        home.install()?;

        // Then the agent's skills folder holds every skill file, as embedded.
        let written = skill_files(&home.path().join(dir))?;
        let embedded: Vec<(String, String)> = SKILLS
            .iter()
            .map(|(file, text)| ((*file).to_owned(), (*text).to_owned()))
            .collect();
        assert_eq!(written, embedded, "every skill file should be written");
        Ok(())
    }

    #[rstest::rstest]
    #[case::micro("run-micro-loop")]
    #[case::phased("run-phased-loop")]
    #[case::simple("run-simple-loop")]
    fn install_puts_the_loop_rules_beside_each_loop(#[case] name: &str) -> TestResult {
        // Given an empty home.
        let home = Home::new()?;

        // When installing.
        home.install()?;

        // Then the loop's folder holds the shared loop rules next to its skill.
        let folder = home.path().join(".claude/skills").join(name);
        let found = (
            folder.join("SKILL.md").is_file(),
            read(&folder.join("task-loop-rules.md"))?,
        );
        assert_eq!(
            found,
            (true, LOOP_RULES.to_owned()),
            "the loop rules should sit beside the loop's SKILL.md"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn install_removes_the_old_pi_copy_of_the_orb_skill() -> TestResult {
        // Given an older orb's copy of the orb skill in pi's agent dir.
        let home = Home::new()?;
        let old = home.path().join(".pi/agent/skills/orb");
        fs::create_dir_all(&old).change_context(IntegrationError)?;
        fs::write(old.join("SKILL.md"), "old").change_context(IntegrationError)?;

        // When installing.
        let lines = home.install()?;

        // Then the old copy is gone and the output says so.
        let says_so = lines
            .iter()
            .any(|line| line == "removed ~/.pi/agent/skills/orb");
        assert_eq!(
            (old.exists(), says_so),
            (false, true),
            "the old pi copy should be removed and reported"
        );
        Ok(())
    }

    /// Makes pi's skills folder a link to the agents skills folder, with an
    /// older orb skill already there, and returns that orb skill's folder.
    fn link_pi_skills_to_agents_skills(home: &Home) -> Result<PathBuf, Report<IntegrationError>> {
        let live = home.paths.agents_skills_dir.join("orb");
        fs::create_dir_all(&live).change_context(IntegrationError)?;
        fs::write(live.join("SKILL.md"), "old").change_context(IntegrationError)?;
        fs::create_dir_all(&home.paths.pi_dir).change_context(IntegrationError)?;
        std::os::unix::fs::symlink(
            &home.paths.agents_skills_dir,
            home.paths.pi_dir.join("skills"),
        )
        .change_context(IntegrationError)?;
        Ok(live)
    }

    #[rstest::rstest]
    fn install_keeps_the_orb_skill_when_pi_skills_links_to_agents_skills() -> TestResult {
        // Given pi's skills folder is a link to the agents skills folder, which
        // already holds an orb skill.
        let home = Home::new()?;
        let live = link_pi_skills_to_agents_skills(&home)?;

        // When installing.
        home.install()?;

        // Then the agents skills folder holds the new orb skill.
        assert_eq!(
            read(&live.join("SKILL.md")).ok().as_deref(),
            skill("orb/SKILL.md"),
            "the orb skill should survive the old-copy removal"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn install_reports_no_removal_when_pi_skills_links_to_agents_skills() -> TestResult {
        // Given pi's skills folder is a link to the agents skills folder, which
        // already holds an orb skill.
        let home = Home::new()?;
        link_pi_skills_to_agents_skills(&home)?;

        // When installing.
        let lines = home.install()?;

        // Then nothing is reported removed.
        let removed = lines.iter().any(|line| line.starts_with("removed "));
        assert!(!removed, "the live orb skill shouldn't be reported removed");
        Ok(())
    }

    #[rstest::rstest]
    fn install_keeps_extra_files_in_the_orb_skill_when_pi_skills_links_to_agents_skills()
    -> TestResult {
        // Given pi's skills folder links to the agents skills folder, whose orb
        // skill holds a file of the user's.
        let home = Home::new()?;
        let live = link_pi_skills_to_agents_skills(&home)?;
        fs::write(live.join("notes.md"), "mine").change_context(IntegrationError)?;

        // When installing.
        home.install()?;

        // Then the user's file is still there.
        assert_eq!(
            read(&live.join("notes.md")).ok().as_deref(),
            Some("mine"),
            "the user's file in the live orb skill should stay"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn install_without_an_old_pi_copy_succeeds_quietly() -> TestResult {
        // Given an empty home, with no old pi copy of the orb skill.
        let home = Home::new()?;

        // When installing.
        let lines = home.install()?;

        // Then nothing is reported removed.
        let removed = lines.iter().any(|line| line.starts_with("removed "));
        assert!(!removed, "nothing should be reported removed");
        Ok(())
    }

    #[rstest::rstest]
    fn install_reports_one_line_per_skills_folder() -> TestResult {
        // Given an empty home.
        let home = Home::new()?;

        // When installing.
        let lines = home.install()?;

        // Then the output names each skills folder with the skill count.
        let skill_lines: Vec<&str> = lines
            .iter()
            .map(String::as_str)
            .filter(|line| line.contains("skills to "))
            .collect();
        assert_eq!(
            skill_lines,
            [
                "wrote 19 skills to ~/.claude/skills",
                "wrote 19 skills to ~/.agents/skills"
            ],
            "the output should have one line per skills folder"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[case::claude(".claude/skills")]
    #[case::pi(".agents/skills")]
    fn install_twice_leaves_the_same_skill_files(#[case] dir: &str) -> TestResult {
        // Given orb's skills already installed.
        let home = Home::new()?;
        home.install()?;
        let before = skill_files(&home.path().join(dir))?;

        // When installing again.
        home.install()?;

        // Then the skill files are as they were.
        let after = skill_files(&home.path().join(dir))?;
        assert_eq!(
            after, before,
            "a second install should leave the same files"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn install_adds_orbs_hooks_to_the_settings_file() -> TestResult {
        // Given an empty home.
        let home = Home::new()?;

        // When installing.
        home.install()?;

        // Then Claude's SessionEnd hook runs orb's script.
        let settings: Value = serde_json::from_str(&read(&home.paths.claude_settings())?)
            .change_context(IntegrationError)?;
        let command = settings
            .pointer("/hooks/SessionEnd/0/hooks/0/command")
            .cloned();
        let expected = claude_settings::hook_command(&home.paths.hook_script());
        assert_eq!(
            command,
            Some(Value::String(expected)),
            "SessionEnd should run orb's hook script"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn install_twice_leaves_the_settings_file_unchanged() -> TestResult {
        // Given orb's integrations already installed.
        let home = Home::new()?;
        home.install()?;
        let before = read(&home.paths.claude_settings())?;

        // When installing again.
        let lines = home.install()?;

        // Then the settings file is as it was and the output says so.
        let after = read(&home.paths.claude_settings())?;
        let says_so = lines
            .iter()
            .any(|line| line == "~/.claude/settings.json already has orb's hooks");
        assert_eq!(
            (after == before, says_so),
            (true, true),
            "a second install should leave the settings alone and say so"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn install_writes_through_a_symlinked_settings_file() -> TestResult {
        // Given Claude's settings.json is a link to a file elsewhere.
        let home = Home::new()?;
        let target = home.path().join("dotfiles/settings.json");
        fs::create_dir_all(home.path().join("dotfiles")).change_context(IntegrationError)?;
        fs::write(&target, "{}\n").change_context(IntegrationError)?;
        fs::create_dir_all(&home.paths.claude_dir).change_context(IntegrationError)?;
        std::os::unix::fs::symlink(&target, home.paths.claude_settings())
            .change_context(IntegrationError)?;

        // When installing.
        home.install()?;

        // Then the link is still a link and its target holds the hooks.
        let still_link = fs::symlink_metadata(home.paths.claude_settings())
            .change_context(IntegrationError)?
            .file_type()
            .is_symlink();
        let target_has_hooks = read(&target)?.contains("orb-agent-state.sh");
        assert_eq!(
            (still_link, target_has_hooks),
            (true, true),
            "the link's target should be written, not the link"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn install_keeps_the_settings_files_mode() -> TestResult {
        // Given a settings file only its owner can read.
        let home = Home::new()?;
        fs::create_dir_all(&home.paths.claude_dir).change_context(IntegrationError)?;
        fs::write(home.paths.claude_settings(), "{}\n").change_context(IntegrationError)?;
        fs::set_permissions(
            home.paths.claude_settings(),
            fs::Permissions::from_mode(0o600),
        )
        .change_context(IntegrationError)?;

        // When installing.
        home.install()?;

        // Then the rewritten file is still owner-only.
        let mode = fs::metadata(home.paths.claude_settings())
            .change_context(IntegrationError)?
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "the settings file should keep its mode");
        Ok(())
    }

    #[rstest::rstest]
    fn install_leaves_settings_with_comments_untouched() -> TestResult {
        // Given a settings file with a comment.
        let home = Home::new()?;
        let text = "{\n  // note\n  \"model\": \"x\"\n}\n";
        fs::create_dir_all(&home.paths.claude_dir).change_context(IntegrationError)?;
        fs::write(home.paths.claude_settings(), text).change_context(IntegrationError)?;

        // When installing.
        let result = home.install();

        // Then the install fails and the file is unchanged.
        let after = read(&home.paths.claude_settings())?;
        assert_eq!(
            (result.is_err(), after.as_str()),
            (true, text),
            "a settings file with comments should be refused and left alone"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn claude_dir_is_claude_config_dir_when_set() {
        // Given CLAUDE_CONFIG_DIR is set.
        let config_dir = PathBuf::from("/cfg/claude");

        // When picking Claude's config home.
        let dir = claude_dir(Some(config_dir.clone()), Path::new("/home/u"));

        // Then it is that folder.
        assert_eq!(dir, config_dir, "CLAUDE_CONFIG_DIR should win");
    }

    #[rstest::rstest]
    fn claude_dir_defaults_to_dot_claude_in_home() {
        // Given CLAUDE_CONFIG_DIR is unset.

        // When picking Claude's config home.
        let dir = claude_dir(None, Path::new("/home/u"));

        // Then it is ~/.claude.
        assert_eq!(
            dir,
            PathBuf::from("/home/u/.claude"),
            "the default is ~/.claude"
        );
    }

    #[rstest::rstest]
    fn pi_agent_dir_is_pi_coding_agent_dir_when_set() {
        // Given PI_CODING_AGENT_DIR is set.
        let agent_dir = PathBuf::from("/cfg/pi");

        // When picking pi's agent dir.
        let dir = pi_agent_dir(Some(agent_dir.clone()), Path::new("/home/u"));

        // Then it is that folder.
        assert_eq!(dir, agent_dir, "PI_CODING_AGENT_DIR should win");
    }

    #[rstest::rstest]
    fn pi_agent_dir_defaults_to_dot_pi_agent_in_home() {
        // Given PI_CODING_AGENT_DIR is unset.

        // When picking pi's agent dir.
        let dir = pi_agent_dir(None, Path::new("/home/u"));

        // Then it is ~/.pi/agent.
        assert_eq!(
            dir,
            PathBuf::from("/home/u/.pi/agent"),
            "the default is ~/.pi/agent"
        );
    }

    #[rstest::rstest]
    fn any_installed_is_false_with_neither_file() -> TestResult {
        // Given an empty home.
        let home = Home::new()?;

        // When asking whether an integration is installed.
        let installed = home.paths.any_installed();

        // Then none is.
        assert!(!installed, "nothing should count as installed");
        Ok(())
    }

    #[rstest::rstest]
    fn any_installed_is_true_with_only_the_pi_extension() -> TestResult {
        // Given only the pi extension in place.
        let home = Home::new()?;
        let extension = home.paths.pi_extension();
        if let Some(parent) = extension.parent() {
            fs::create_dir_all(parent).change_context(IntegrationError)?;
        }
        fs::write(&extension, "").change_context(IntegrationError)?;

        // When asking whether an integration is installed.
        let installed = home.paths.any_installed();

        // Then one is.
        assert!(installed, "the pi extension alone should count");
        Ok(())
    }
}
