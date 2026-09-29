//! Workspace trust — telling Claude the user trusts a folder.
//!
//! Claude refuses to start a session in a folder the user hasn't trusted.
//! When the user says yes in orb, orb records that answer in Claude's global
//! config the way Claude's own prompt would, so the next start goes through
//! and every worktree of the same repository is trusted with it.

use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use error_stack::{Report, ResultExt};
use serde_json::{Map, Value};
use unicode_normalization::UnicodeNormalization;
use wherror::Error;

/// How many times a write is redone because Claude changed the file meanwhile.
const ATTEMPTS: usize = 5;

/// Trusting a folder failed. The report says why.
#[derive(Debug, Error)]
#[error(debug)]
pub struct WorkspaceTrustError;

/// Records that the user trusts a folder.
pub trait WorkspaceTrust: Send + Sync {
    fn name(&self) -> &'static str;

    /// Marks `dir` trusted, keeping everything else as it was.
    ///
    /// # Errors
    ///
    /// Returns an error if the trust couldn't be saved.
    fn trust(&self, dir: &Path) -> Result<(), Report<WorkspaceTrustError>>;
}

/// Shared handle to the [`WorkspaceTrust`] in use.
#[derive(Clone)]
pub struct WorkspaceTrustService {
    trust: Arc<dyn WorkspaceTrust>,
}

impl WorkspaceTrustService {
    pub fn new(trust: Arc<dyn WorkspaceTrust>) -> Self {
        Self { trust }
    }

    /// Marks `dir` trusted.
    ///
    /// # Errors
    ///
    /// Returns an error if the trust couldn't be saved.
    pub fn trust(&self, dir: &Path) -> Result<(), Report<WorkspaceTrustError>> {
        self.trust.trust(dir)
    }
}

impl fmt::Debug for WorkspaceTrustService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "WorkspaceTrust<{}>", self.trust.name())
    }
}

/// Claude's global config file, `.claude.json`, as the record of trust.
#[derive(Debug, Clone)]
pub struct ClaudeConfigTrust {
    file: PathBuf,
}

impl ClaudeConfigTrust {
    pub fn new(file: PathBuf) -> Self {
        Self { file }
    }
}

impl WorkspaceTrust for ClaudeConfigTrust {
    fn name(&self) -> &'static str {
        "claude-config"
    }

    fn trust(&self, dir: &Path) -> Result<(), Report<WorkspaceTrustError>> {
        let key = project_key(dir)?;
        let temp = {
            let mut name = self.file.clone().into_os_string();
            name.push(".orb-tmp");
            PathBuf::from(name)
        };
        // ponytail: the stamp check narrows the race with a Claude saving the
        // same file but can't close it (Claude takes no lock); a lost entry
        // shows as a second refusal.
        for _ in 0..ATTEMPTS {
            let before = stamp(&self.file);
            let text = read(&self.file)?;
            write_private(&temp, &trusted(text.as_deref(), &key)?)?;
            if stamp(&self.file) == before {
                return fs::rename(&temp, &self.file)
                    .change_context(WorkspaceTrustError)
                    .attach("couldn't replace Claude's config");
            }
            let _ = fs::remove_file(&temp);
        }
        Err(Report::new(WorkspaceTrustError).attach("Claude's config kept changing"))
    }
}

/// The file Claude 2.1.284 keeps its global config in: `.config.json` in its
/// config directory when that exists, else `.claude.json` in
/// `CLAUDE_CONFIG_DIR`, else in the home directory.
pub fn claude_config_file(config_dir: Option<&Path>, home: &Path) -> PathBuf {
    let legacy = config_dir
        .map_or_else(|| home.join(".claude"), Path::to_owned)
        .join(".config.json");
    match (legacy.exists(), config_dir) {
        (true, _) => legacy,
        (false, Some(dir)) => dir.join(".claude.json"),
        (false, None) => home.join(".claude.json"),
    }
}

/// Claude's key for `dir`: its real path, NFC-normalized.
fn project_key(dir: &Path) -> Result<String, Report<WorkspaceTrustError>> {
    let real = fs::canonicalize(dir)
        .change_context(WorkspaceTrustError)
        .attach("couldn't resolve the folder")?;
    let text = real
        .to_str()
        .ok_or_else(|| Report::new(WorkspaceTrustError).attach("the folder's path isn't UTF-8"))?;
    Ok(text.nfc().collect())
}

/// What tells a changed file apart: its modification time and size.
fn stamp(file: &Path) -> Option<(Option<SystemTime>, u64)> {
    fs::metadata(file)
        .ok()
        .map(|m| (m.modified().ok(), m.len()))
}

/// The config's text, `None` when there is no file yet.
fn read(file: &Path) -> Result<Option<String>, Report<WorkspaceTrustError>> {
    match fs::read_to_string(file) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Report::new(e)
            .change_context(WorkspaceTrustError)
            .attach("couldn't read Claude's config")),
    }
}

/// The config `text` with `key` marked trusted, in Claude's format.
fn trusted(text: Option<&str>, key: &str) -> Result<String, Report<WorkspaceTrustError>> {
    let mut root = match text {
        Some(text) => serde_json::from_str(text)
            .change_context(WorkspaceTrustError)
            .attach("Claude's config isn't valid JSON")?,
        None => Value::Object(Map::new()),
    };
    let project = child(child(object(&mut root)?, "projects")?, key)?;
    project.insert("hasTrustDialogAccepted".to_owned(), Value::Bool(true));
    serde_json::to_string_pretty(&root)
        .change_context(WorkspaceTrustError)
        .attach("couldn't write Claude's config")
}

/// `value` as an object, or an error when it's something else.
fn object(value: &mut Value) -> Result<&mut Map<String, Value>, Report<WorkspaceTrustError>> {
    value.as_object_mut().ok_or_else(|| {
        Report::new(WorkspaceTrustError).attach("Claude's config isn't shaped as expected")
    })
}

/// The object at `key` in `map`, made empty when missing.
fn child<'a>(
    map: &'a mut Map<String, Value>,
    key: &str,
) -> Result<&'a mut Map<String, Value>, Report<WorkspaceTrustError>> {
    object(map.entry(key).or_insert_with(|| Value::Object(Map::new())))
}

/// Writes `text` to `temp`, readable by the user only.
fn write_private(temp: &Path, text: &str) -> Result<(), Report<WorkspaceTrustError>> {
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(temp)
        .change_context(WorkspaceTrustError)
        .attach("couldn't write Claude's config")?;
    file.write_all(text.as_bytes())
        .and_then(|()| file.sync_all())
        .change_context(WorkspaceTrustError)
        .attach("couldn't write Claude's config")
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "tests set files up with `?` and assert on the outcome"
)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    use error_stack::{Report, ResultExt};
    use serde_json::{Value, json};
    use tempfile::TempDir;

    use super::{ClaudeConfigTrust, WorkspaceTrust, WorkspaceTrustError, claude_config_file};

    type TestResult = Result<(), Report<WorkspaceTrustError>>;

    /// A project folder and a config file path in a scratch directory.
    struct Fixture {
        _tmp: TempDir,
        project: PathBuf,
        file: PathBuf,
        key: String,
    }

    impl Fixture {
        fn new() -> Result<Self, Report<WorkspaceTrustError>> {
            let tmp = TempDir::new().change_context(WorkspaceTrustError)?;
            let project = tmp.path().join("project");
            fs::create_dir_all(&project).change_context(WorkspaceTrustError)?;
            let key = project
                .canonicalize()
                .change_context(WorkspaceTrustError)?
                .display()
                .to_string();
            let file = tmp.path().join(".claude.json");
            Ok(Self {
                _tmp: tmp,
                project,
                file,
                key,
            })
        }

        fn write(&self, text: &str) -> TestResult {
            fs::write(&self.file, text).change_context(WorkspaceTrustError)
        }

        fn text(&self) -> Result<String, Report<WorkspaceTrustError>> {
            fs::read_to_string(&self.file).change_context(WorkspaceTrustError)
        }

        fn json(&self) -> Result<Value, Report<WorkspaceTrustError>> {
            serde_json::from_str(&self.text()?).change_context(WorkspaceTrustError)
        }

        fn trust(&self, dir: &Path) -> TestResult {
            ClaudeConfigTrust::new(self.file.clone()).trust(dir)
        }
    }

    #[rstest::rstest]
    fn trusting_a_folder_marks_it_trusted() -> TestResult {
        // Given a config with other keys and another project.
        let fx = Fixture::new()?;
        fx.write(r#"{"numStartups": 3, "projects": {"/other": {"allowedTools": []}}}"#)?;

        // When trusting the project.
        fx.trust(&fx.project)?;

        // Then its entry has hasTrustDialogAccepted set.
        let accepted = fx
            .json()?
            .get("projects")
            .and_then(|p| p.get(&fx.key))
            .and_then(|e| e.get("hasTrustDialogAccepted"))
            .cloned();
        assert_eq!(
            accepted,
            Some(Value::Bool(true)),
            "the project should be trusted"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn trusting_a_folder_leaves_the_rest_of_the_file_as_it_was() -> TestResult {
        // Given a config in Claude's format with unsorted keys, non-ASCII text
        // and another project.
        let fx = Fixture::new()?;
        let head = "{\n  \"numStartups\": 3,\n  \"autoUpdates\": false,\n  \"name\": \"café\",\n  \"tips\": [\n    \"a\",\n    \"b\"\n  ],\n  \"projects\": {\n    \"/other\": {\n      \"allowedTools\": [],\n      \"mcpServers\": {}\n    }";
        let tail = "\n  },\n  \"userID\": \"u1\"\n}";
        fx.write(&format!("{head}{tail}"))?;

        // When trusting the project.
        fx.trust(&fx.project)?;

        // Then the file is unchanged apart from the new entry at the end of
        // `projects`.
        let expected = format!(
            "{head},\n    \"{}\": {{\n      \"hasTrustDialogAccepted\": true\n    }}{tail}",
            fx.key
        );
        assert_eq!(fx.text()?, expected, "only the new entry should be added");
        Ok(())
    }

    #[rstest::rstest]
    fn trusting_a_project_keeps_its_other_fields() -> TestResult {
        // Given the project already has an untrusted entry with other fields.
        let fx = Fixture::new()?;
        let config = json!({"projects": {fx.key.as_str(): {
            "allowedTools": ["Bash"],
            "hasTrustDialogAccepted": false,
            "lastSessionId": "s1"
        }}});
        fx.write(&config.to_string())?;

        // When trusting the project.
        fx.trust(&fx.project)?;

        // Then only hasTrustDialogAccepted changed.
        let entry = fx
            .json()?
            .get("projects")
            .and_then(|p| p.get(&fx.key))
            .cloned();
        let expected = json!({
            "allowedTools": ["Bash"],
            "hasTrustDialogAccepted": true,
            "lastSessionId": "s1"
        });
        assert_eq!(
            entry,
            Some(expected),
            "the entry's other fields should stay"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn trusting_without_a_config_file_creates_it() -> TestResult {
        // Given no config file.
        let fx = Fixture::new()?;

        // When trusting the project.
        fx.trust(&fx.project)?;

        // Then the file holds just the trusted entry.
        let expected = json!({"projects": {fx.key.as_str(): {"hasTrustDialogAccepted": true}}});
        assert_eq!(fx.json()?, expected, "a new config should hold the entry");
        Ok(())
    }

    #[rstest::rstest]
    fn trusting_an_nfd_named_folder_writes_its_nfc_key() -> TestResult {
        // Given a folder whose name is NFD.
        let fx = Fixture::new()?;
        let parent = fx.project.parent().ok_or(WorkspaceTrustError)?;
        let dir = parent.join("cafe\u{301}");
        fs::create_dir_all(&dir).change_context(WorkspaceTrustError)?;

        // When trusting it.
        fx.trust(&dir)?;

        // Then the key is the NFC form of its real path.
        let key = format!(
            "{}/caf\u{e9}",
            parent
                .canonicalize()
                .change_context(WorkspaceTrustError)?
                .display()
        );
        let has_key = fx
            .json()?
            .get("projects")
            .and_then(Value::as_object)
            .is_some_and(|p| p.contains_key(&key));
        assert!(has_key, "the key should be NFC");
        Ok(())
    }

    #[rstest::rstest]
    fn trusting_over_invalid_json_leaves_the_file_untouched() -> TestResult {
        // Given a config that isn't valid JSON.
        let fx = Fixture::new()?;
        fx.write(r#"{"projects":"#)?;

        // When trusting the project.
        let result = fx.trust(&fx.project);

        // Then trusting fails and the file is as it was.
        assert!(
            result.is_err() && fx.text()? == r#"{"projects":"#,
            "an invalid config should not be overwritten"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn trusted_config_is_private() -> TestResult {
        // Given no config file.
        let fx = Fixture::new()?;

        // When trusting the project.
        fx.trust(&fx.project)?;

        // Then only the user can read or write the file.
        let mode = fs::metadata(&fx.file)
            .change_context(WorkspaceTrustError)?
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "the config should be private");
        Ok(())
    }

    #[rstest::rstest]
    #[case(false, false, ".claude.json")]
    #[case(true, false, "cfg/.claude.json")]
    #[case(true, true, "cfg/.config.json")]
    #[case(false, true, ".claude/.config.json")]
    fn claude_config_file_follows_claudes_rule(
        #[case] set: bool,
        #[case] legacy: bool,
        #[case] expected: &str,
    ) -> TestResult {
        // Given a home, maybe a config dir, and maybe a legacy config file.
        let tmp = TempDir::new().change_context(WorkspaceTrustError)?;
        let home = tmp.path();
        let config_dir = set.then(|| home.join("cfg"));
        if legacy {
            let dir = config_dir.clone().unwrap_or_else(|| home.join(".claude"));
            fs::create_dir_all(&dir).change_context(WorkspaceTrustError)?;
            fs::write(dir.join(".config.json"), "{}").change_context(WorkspaceTrustError)?;
        }

        // When finding the config file.
        let file = claude_config_file(config_dir.as_deref(), home);

        // Then it follows Claude's rule.
        assert_eq!(file, home.join(expected), "config file path");
        Ok(())
    }
}
