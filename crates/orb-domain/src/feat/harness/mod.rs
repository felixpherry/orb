//! Harnesses, the programs a thread runs in.
//!
//! Everything orb does differently per harness (starting and listing
//! sessions, reading transcripts, trusting a folder, the models on offer)
//! lives in that harness's implementation. Each draft, group and thread
//! stores the id of its harness, and shared code picks the harness by it.

pub mod claude;
pub mod pi;

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use error_stack::Report;
use wherror::Error;

use super::sessions::session_host::SessionHost;
use super::sessions::transcript::{Exchange, MessageRead};

/// The id a harness is stored and looked up by.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HarnessId(String);

impl HarnessId {
    pub fn new<S>(id: S) -> Self
    where
        S: Into<String>,
    {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for HarnessId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// What the frontend shows and binds for a harness, published into AppState.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessInfo {
    pub id: HarnessId,
    /// The name the user sees.
    pub label: String,
    /// The tag a thread of this harness shows in the sidebar, if any.
    pub tag: Option<String>,
    /// The mark drawn before the harness's models and replies and at the end
    /// of its threads' nodes, if it has one.
    pub icon: Option<String>,
    /// Why the harness can't start sessions right now, if it can't.
    pub unavailable: Option<String>,
    /// The models on offer; the harness's own default is implied.
    pub models: Vec<ModelGroup>,
    /// The permission modes on offer; empty when the harness has none.
    pub permission_modes: Vec<String>,
    /// The attached program redraws only when its screen size changes, so
    /// orb nudges the size once the attach has connected.
    pub nudge_on_attach: bool,
    /// The escape sequences for the terminal modes the program turns on when
    /// it starts, which an attach to it already running never sees; the pane
    /// takes them in before the attach's output.
    pub attach_modes: &'static [u8],
    /// Something the probe ran into, shown once on the mode line.
    pub notice: Option<String>,
}

/// Why a harness whose probe hasn't answered can't be picked yet.
pub const CHECKING: &str = "checking";

impl HarnessInfo {
    /// What a harness shows before its probe answers: its id and label, not
    /// yet usable, nothing to pick.
    pub fn placeholder(id: HarnessId, label: &str) -> Self {
        Self {
            id,
            label: label.to_owned(),
            tag: None,
            icon: None,
            unavailable: Some(CHECKING.to_owned()),
            models: Vec::new(),
            permission_modes: Vec::new(),
            nudge_on_attach: false,
            attach_modes: &[],
            notice: None,
        }
    }

    /// The model `value` names, by its id or one of its aliases.
    pub fn model(&self, value: &str) -> Option<&ModelChoice> {
        self.models
            .iter()
            .flat_map(|group| &group.models)
            .find(|model| model.id == value || model.aliases.iter().any(|alias| alias == value))
    }
}

/// Models listed together in the model picker, under an optional heading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelGroup {
    pub heading: Option<String>,
    pub models: Vec<ModelChoice>,
}

/// One model the user can start a thread with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelChoice {
    /// What the harness is told to run.
    pub id: String,
    /// The name the user sees.
    pub name: String,
    /// Other values that name this model, such as older stored settings.
    pub aliases: Vec<String>,
}

/// What a transcript scan found since the last offset.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scan {
    /// The generated title, else the first prompt.
    pub title: Option<String>,
    /// The name the user gave the session inside the harness.
    pub custom_title: Option<String>,
    /// The git branch the transcript named; `None` leaves it to git.
    pub branch: Option<String>,
    /// A generated title arrived in the lines scanned.
    pub ai_titled: bool,
    /// Bytes read so far; pass it to the next scan.
    pub offset: u64,
}

/// How a harness writes its transcripts, and how to read them.
pub trait TranscriptFormat: Send + Sync {
    /// The transcript file of `session_id` started in `cwd`, if it exists.
    fn locate(&self, cwd: &Path, session_id: &str) -> Option<PathBuf>;

    /// Scans the lines added since `offset`, building on `previous`.
    ///
    /// # Errors
    ///
    /// Returns an error if the transcript can't be read.
    fn scan(&self, path: &Path, offset: u64, previous: &Scan) -> io::Result<Scan>;

    /// The transcript's length and its last exchanges.
    ///
    /// # Errors
    ///
    /// Returns an error if the transcript can't be read.
    fn exchanges(&self, path: &Path) -> io::Result<(u64, Vec<Exchange>)>;

    /// The prompts and replies added since `offset`.
    ///
    /// # Errors
    ///
    /// Returns an error if the transcript can't be read.
    fn messages(&self, path: &Path, offset: u64, prompt_offset: u64) -> io::Result<MessageRead>;
}

/// A harness step failed.
#[derive(Debug, Error)]
#[error(debug)]
pub struct HarnessError;

/// A program a thread runs in: it hosts the sessions, writes the
/// transcripts, and says what the user can pick when starting one.
#[async_trait]
pub trait Harness: SessionHost + TranscriptFormat {
    fn id(&self) -> HarnessId;

    /// The name shown before `probe` has answered.
    fn label(&self) -> &'static str;

    /// Asks the harness what it offers and whether it can run.
    async fn probe(&self) -> HarnessInfo;

    /// The folder a refused start asks the user to trust.
    fn trust_dir(&self, cwd: &Path) -> PathBuf {
        cwd.to_owned()
    }

    /// Records the user's trust in `dir`.
    ///
    /// # Errors
    ///
    /// Returns an error if the trust can't be recorded.
    fn trust(&self, _dir: &Path) -> Result<(), Report<HarnessError>> {
        Ok(())
    }
}

/// Every harness orb knows, in registration order; the first is the default.
#[derive(Clone)]
pub struct Harnesses {
    all: Vec<Arc<dyn Harness>>,
}

impl Harnesses {
    pub fn new(all: Vec<Arc<dyn Harness>>) -> Self {
        Self { all }
    }

    /// The harness stored as `id`, if orb knows it.
    pub fn get(&self, id: &HarnessId) -> Option<&Arc<dyn Harness>> {
        self.all.iter().find(|harness| harness.id() == *id)
    }

    pub fn all(&self) -> &[Arc<dyn Harness>] {
        &self.all
    }

    /// The first registered harness's id; an empty registry gives an id no
    /// harness answers to.
    pub fn default_id(&self) -> HarnessId {
        self.all
            .first()
            .map_or_else(|| HarnessId::new(""), |harness| harness.id())
    }

    /// A placeholder per harness, in registration order.
    pub fn placeholders(&self) -> Vec<HarnessInfo> {
        self.all
            .iter()
            .map(|harness| HarnessInfo::placeholder(harness.id(), harness.label()))
            .collect()
    }
}

impl fmt::Debug for Harnesses {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<&str> = self.all.iter().map(|harness| harness.name()).collect();
        write!(f, "Harnesses<{}>", names.join(", "))
    }
}

#[cfg(test)]
pub(crate) mod fake {
    use std::ffi::OsString;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use async_trait::async_trait;
    use error_stack::Report;

    use super::{Harness, HarnessId, HarnessInfo, Scan, TranscriptFormat};
    use crate::feat::sessions::session_host::{
        CreatedSession, SessionHost, SessionHostError, SessionOptions, SessionRecord,
    };
    use crate::feat::sessions::transcript::{Exchange, MessageRead};

    /// A pi-like harness's info: tagged `pi`, no mark, available, two
    /// provider groups of models and no permission modes.
    pub(crate) fn pi_like() -> HarnessInfo {
        HarnessInfo {
            tag: Some("pi".to_owned()),
            unavailable: None,
            models: super::pi::models::parse(
                "provider   model\n\
                 anthropic  claude-x\n\
                 anthropic  claude-y\n\
                 openai     gpt-z\n",
            ),
            ..HarnessInfo::placeholder(HarnessId::new("pi"), "pi")
        }
    }

    /// A harness named `id` that hosts through `host` and has no transcripts.
    pub(crate) struct FakeHarness {
        id: &'static str,
        host: Arc<dyn SessionHost>,
    }

    impl FakeHarness {
        pub(crate) fn new(id: &'static str, host: Arc<dyn SessionHost>) -> Self {
            Self { id, host }
        }
    }

    #[async_trait]
    impl SessionHost for FakeHarness {
        fn name(&self) -> &'static str {
            self.host.name()
        }

        async fn create(
            &self,
            cwd: &Path,
            options: &SessionOptions,
        ) -> Result<CreatedSession, Report<SessionHostError>> {
            self.host.create(cwd, options).await
        }

        async fn list(
            &self,
            short_ids: &[String],
        ) -> Result<Vec<SessionRecord>, Report<SessionHostError>> {
            self.host.list(short_ids).await
        }

        async fn stop(&self, short_id: &str) -> Result<(), Report<SessionHostError>> {
            self.host.stop(short_id).await
        }

        async fn remove(&self, short_id: &str) -> Result<(), Report<SessionHostError>> {
            self.host.remove(short_id).await
        }

        /// `[<id>, <short id>]`, so tests can tell which harness built it.
        fn attach_argv(&self, short_id: &str) -> Vec<OsString> {
            vec![OsString::from(self.id), OsString::from(short_id)]
        }
    }

    impl TranscriptFormat for FakeHarness {
        fn locate(&self, _cwd: &Path, _session_id: &str) -> Option<PathBuf> {
            None
        }

        fn scan(&self, _path: &Path, _offset: u64, previous: &Scan) -> io::Result<Scan> {
            Ok(previous.clone())
        }

        fn exchanges(&self, _path: &Path) -> io::Result<(u64, Vec<Exchange>)> {
            Ok((0, Vec::new()))
        }

        fn messages(
            &self,
            _path: &Path,
            offset: u64,
            prompt_offset: u64,
        ) -> io::Result<MessageRead> {
            Ok(MessageRead {
                messages: Vec::new(),
                offset,
                restarted: false,
                prompt_offset,
            })
        }
    }

    #[async_trait]
    impl Harness for FakeHarness {
        fn id(&self) -> HarnessId {
            HarnessId::new(self.id)
        }

        fn label(&self) -> &'static str {
            self.id
        }

        async fn probe(&self) -> HarnessInfo {
            HarnessInfo {
                unavailable: None,
                ..HarnessInfo::placeholder(self.id(), self.id)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::claude::supervisor::ClaudeSupervisor;
    use super::fake::FakeHarness;
    use super::{CHECKING, HarnessId, HarnessInfo, Harnesses};

    #[rstest::rstest]
    fn default_id_is_the_first_registered_harness() {
        // Given two harnesses, `first` registered before `second`.
        let host = Arc::new(ClaudeSupervisor::new(Vec::new()));
        let harnesses = Harnesses::new(vec![
            Arc::new(FakeHarness::new("first", host.clone())),
            Arc::new(FakeHarness::new("second", host)),
        ]);

        // When asking for the default.
        let default = harnesses.default_id();

        // Then it is the first one registered.
        assert_eq!(
            default,
            HarnessId::new("first"),
            "the first registered harness should be the default"
        );
    }

    #[rstest::rstest]
    fn placeholder_is_checking_until_its_probe_answers() {
        // Given nothing but a harness's id and label.
        let id = HarnessId::new("pi");

        // When building its placeholder.
        let info = HarnessInfo::placeholder(id, "pi");

        // Then it can't be picked yet, because it is still being checked.
        assert_eq!(
            info.unavailable.as_deref(),
            Some(CHECKING),
            "an unprobed harness should be unavailable while checking"
        );
    }
}
