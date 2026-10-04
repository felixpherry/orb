//! Claude Code's background supervisor as orb's session host.
//!
//! `claude --bg` starts an idle session, with `--model` and
//! `--permission-mode` when a session asks for them, `claude agents --json --all` reports
//! what every session is doing, `claude stop <id>` stops one and keeps its
//! conversation, `claude rm <id>` deletes one, and `claude attach <id>`
//! attaches to one, resuming it if it was stopped. Every `claude` process runs
//! with orb's child environment and no stdin, and is killed if it outlives its
//! time limit. When `claude` fails, the first line it printed becomes the
//! reason.

use std::ffi::OsString;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use error_stack::{Report, ResultExt};
use serde::Deserialize;
use tokio::process::Command;
use tokio::time::timeout;

use crate::feat::sessions::session_host::{
    CreatedSession, SessionHost, SessionHostError, SessionOptions, SessionRecord,
    WorkspaceUntrusted,
};
use crate::feat::sessions::state::ThreadStatus;

const CREATE_TIMEOUT: Duration = Duration::from_secs(30);
const LIST_TIMEOUT: Duration = Duration::from_secs(10);
/// How long `claude stop` and `claude rm` may take.
const STOP_TIMEOUT: Duration = Duration::from_secs(10);

/// `waitingFor` values that mean Claude waits for an approval.
const APPROVALS: [&str; 3] = ["permission prompt", "sandbox request", "worker request"];

/// What `claude --bg` prints to stderr before the session's id.
const BANNER: &str = "Starting background service";
/// What `claude` prints when it refuses a directory it hasn't been trusted in.
const UNTRUSTED: &str = "Workspace not trusted";

/// Hosts sessions in Claude Code's background supervisor.
#[derive(Debug, Clone)]
pub struct ClaudeSupervisor {
    /// The environment every `claude` process gets.
    env: Vec<(OsString, OsString)>,
}

impl ClaudeSupervisor {
    /// A supervisor whose `claude` processes run with exactly `env`.
    pub fn new(env: Vec<(OsString, OsString)>) -> Self {
        Self { env }
    }

    fn claude(&self, args: &[&str]) -> Command {
        let mut command = Command::new("claude");
        command
            .args(args)
            .env_clear()
            .envs(self.env.iter().map(|(key, value)| (key, value)))
            .stdin(Stdio::null())
            .kill_on_drop(true);
        command
    }
}

#[async_trait]
impl SessionHost for ClaudeSupervisor {
    fn name(&self) -> &'static str {
        "claude"
    }

    async fn create(
        &self,
        cwd: &Path,
        options: &SessionOptions,
    ) -> Result<CreatedSession, Report<SessionHostError>> {
        let command = {
            let mut command = self.claude(&bg_args(options));
            command.current_dir(cwd);
            command
        };
        let text = run(command, "claude --bg", CREATE_TIMEOUT).await?;
        let short_id = parse_backgrounded(&text)?;
        Ok(CreatedSession { short_id })
    }

    /// `claude agents --all` lists every session, so the ids aren't needed.
    async fn list(
        &self,
        _short_ids: &[String],
    ) -> Result<Vec<SessionRecord>, Report<SessionHostError>> {
        let command = self.claude(&["agents", "--json", "--all"]);
        let text = run(command, "claude agents", LIST_TIMEOUT).await?;
        parse_agents(&text)
    }

    async fn stop(&self, short_id: &str) -> Result<(), Report<SessionHostError>> {
        run(
            self.claude(&["stop", short_id]),
            "claude stop",
            STOP_TIMEOUT,
        )
        .await?;
        Ok(())
    }

    async fn remove(&self, short_id: &str) -> Result<(), Report<SessionHostError>> {
        run(self.claude(&["rm", short_id]), "claude rm", STOP_TIMEOUT).await?;
        Ok(())
    }

    fn attach_argv(&self, short_id: &str) -> Vec<OsString> {
        ["claude", "attach", short_id].map(OsString::from).into()
    }
}

/// The arguments of `claude` that start an idle session with `options`.
fn bg_args(options: &SessionOptions) -> Vec<&str> {
    let model = options.model.as_deref().map(|model| ["--model", model]);
    let mode = options
        .permission_mode
        .as_deref()
        .map(|mode| ["--permission-mode", mode]);
    ["--bg"]
        .into_iter()
        .chain(model.into_iter().flatten())
        .chain(mode.into_iter().flatten())
        .collect()
}

/// Runs `command` for at most `limit`; returns its stdout, then its stderr.
async fn run(
    mut command: Command,
    what: &str,
    limit: Duration,
) -> Result<String, Report<SessionHostError>> {
    let output = timeout(limit, command.output())
        .await
        .change_context(SessionHostError)
        .attach(format!("{what} timed out after {} s", limit.as_secs()))?
        .map_err(|error| {
            let reason = format!("couldn't run {what}: {error}");
            Report::new(error)
                .change_context(SessionHostError)
                .attach(reason)
        })?;
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if output.status.success() {
        Ok(text)
    } else {
        Err(failure(
            &text,
            &format!("{what} exited with {}", output.status),
        ))
    }
}

/// A report whose reason is the first line `claude` printed (skipping the
/// startup banner), else `fallback`. A refused untrusted directory is also
/// marked [`WorkspaceUntrusted`].
fn failure(text: &str, fallback: &str) -> Report<SessionHostError> {
    let reason = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with(BANNER))
        .unwrap_or(fallback);
    let report = Report::new(SessionHostError);
    let report = if text.contains(UNTRUSTED) {
        report.attach_opaque(WorkspaceUntrusted)
    } else {
        report
    };
    report.attach(reason.to_owned())
}

/// The short id in `claude --bg`'s `backgrounded · <id> …` line.
fn parse_backgrounded(text: &str) -> Result<String, Report<SessionHostError>> {
    text.split_once("backgrounded · ")
        .and_then(|(_, rest)| rest.split_whitespace().next())
        .map(|token| token.trim_matches(|c: char| !c.is_ascii_alphanumeric()))
        .filter(|id| !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .map(str::to_owned)
        .ok_or_else(|| failure(text, "claude --bg printed no session id"))
}

/// One record of `claude agents --json`; unknown fields are ignored.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentRecord {
    id: Option<String>,
    session_id: Option<String>,
    status: Option<String>,
    waiting_for: Option<String>,
    state: Option<String>,
}

/// The sessions in `claude agents --json`'s array. Records without an `id`
/// (interactive sessions) can't be attached to and are skipped.
fn parse_agents(json: &str) -> Result<Vec<SessionRecord>, Report<SessionHostError>> {
    let records: Vec<AgentRecord> = serde_json::from_str(json)
        .change_context(SessionHostError)
        .attach("claude agents printed unreadable JSON".to_owned())?;
    Ok(records
        .into_iter()
        .filter_map(|record| {
            let status = status_of(&record);
            Some(SessionRecord {
                short_id: record.id?,
                session_id: record.session_id,
                status,
            })
        })
        .collect())
}

fn status_of(record: &AgentRecord) -> ThreadStatus {
    match (record.status.as_deref(), record.state.as_deref()) {
        (Some("busy"), _) => ThreadStatus::Working,
        (Some("waiting"), _) => match record.waiting_for.as_deref() {
            Some(waiting_for) if APPROVALS.contains(&waiting_for) => ThreadStatus::NeedsApproval,
            _ => ThreadStatus::NeedsInput,
        },
        (_, Some("failed")) => ThreadStatus::Failed,
        (Some(_), _) => ThreadStatus::Idle,
        (None, _) => ThreadStatus::Stopped,
    }
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "tests propagate parse failures with `?` and assert on the outcome"
)]
mod tests {
    use std::ffi::OsString;

    use error_stack::Report;

    use super::{
        ClaudeSupervisor, SessionHostError, SessionOptions, ThreadStatus, WorkspaceUntrusted,
        bg_args, parse_agents, parse_backgrounded,
    };
    use crate::feat::sessions::session_host::SessionHost;

    #[rstest::rstest]
    fn default_options_start_with_only_bg() {
        // Given options that leave model and permission to Claude.
        let options = SessionOptions::default();

        // When building the start arguments.
        let args = bg_args(&options);

        // Then only `--bg` is passed.
        assert_eq!(args, ["--bg"], "defaults should pass no flags");
    }

    #[rstest::rstest]
    fn model_and_permission_mode_are_passed_as_flags() {
        // Given sonnet in plan mode.
        let options = SessionOptions {
            model: Some("sonnet".to_owned()),
            permission_mode: Some("plan".to_owned()),
        };

        // When building the start arguments.
        let args = bg_args(&options);

        // Then both flags follow `--bg`.
        assert_eq!(
            args,
            ["--bg", "--model", "sonnet", "--permission-mode", "plan"],
            "the model and permission mode should be passed"
        );
    }

    #[rstest::rstest]
    fn backgrounded_line_with_a_name_yields_the_short_id() -> Result<(), Report<SessionHostError>> {
        // Given `claude --bg -n orb-m2-probe`'s output.
        let text = "backgrounded · 28bf38e2 · orb-m2-probe (idle — send a prompt to start)\n\
                    \nStarting background service…\n";

        // When parsing it.
        let short_id = parse_backgrounded(text)?;

        // Then the short id is the token after `backgrounded · `.
        assert_eq!(
            short_id, "28bf38e2",
            "the named session's id should be parsed"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn backgrounded_line_without_a_name_yields_the_short_id() -> Result<(), Report<SessionHostError>>
    {
        // Given `claude --bg`'s output, help lines included.
        let text = "backgrounded · baf42bd2 (idle — send a prompt to start)\n  \
                    claude agents             list sessions\n  \
                    claude attach baf42bd2    open in this terminal\n\
                    \nStarting background service…\n";

        // When parsing it.
        let short_id = parse_backgrounded(text)?;

        // Then the short id is the token after `backgrounded · `.
        assert_eq!(
            short_id, "baf42bd2",
            "the unnamed session's id should be parsed"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn untrusted_refusal_carries_the_untrusted_marker() {
        // Given `claude --bg`'s refusal in an untrusted directory.
        let text = "\nStarting background service…\n\
                    Workspace not trusted. Run `claude` in /tmp/x once and accept the trust prompt\n";

        // When parsing it.
        let result = parse_backgrounded(text);

        // Then the error is marked as an untrusted workspace.
        assert!(
            result
                .err()
                .is_some_and(|report| report.contains::<WorkspaceUntrusted>()),
            "the refusal should be marked untrusted"
        );
    }

    #[rstest::rstest]
    fn untrusted_refusal_keeps_its_reason() {
        // Given `claude --bg`'s refusal in an untrusted directory.
        let refusal =
            "Workspace not trusted. Run `claude` in /tmp/x once and accept the trust prompt";
        let text = format!("\nStarting background service…\n{refusal}\n");

        // When parsing it.
        let result = parse_backgrounded(&text);

        // Then the error's reason is the refusal line.
        let reason = result
            .err()
            .and_then(|report| report.downcast_ref::<String>().cloned());
        assert_eq!(
            reason.as_deref(),
            Some(refusal),
            "the refusal should be the reason"
        );
    }

    #[rstest::rstest]
    #[case(
        r#"{"id":"28bf38e2","status":"busy","state":"working"}"#,
        ThreadStatus::Working
    )]
    #[case(
        r#"{"id":"28bf38e2","status":"waiting","waitingFor":"permission prompt"}"#,
        ThreadStatus::NeedsApproval
    )]
    #[case(
        r#"{"id":"28bf38e2","status":"waiting","waitingFor":"sandbox request"}"#,
        ThreadStatus::NeedsApproval
    )]
    #[case(
        r#"{"id":"28bf38e2","status":"waiting","waitingFor":"worker request"}"#,
        ThreadStatus::NeedsApproval
    )]
    #[case(
        r#"{"id":"28bf38e2","status":"waiting","waitingFor":"input needed"}"#,
        ThreadStatus::NeedsInput
    )]
    #[case(
        r#"{"id":"28bf38e2","status":"waiting","waitingFor":"dialog open"}"#,
        ThreadStatus::NeedsInput
    )]
    #[case(r#"{"id":"28bf38e2","status":"waiting"}"#, ThreadStatus::NeedsInput)]
    #[case(
        r#"{"id":"28bf38e2","status":"idle","state":"failed"}"#,
        ThreadStatus::Failed
    )]
    #[case(
        r#"{"id":"28bf38e2","status":"idle","state":"blocked"}"#,
        ThreadStatus::Idle
    )]
    #[case(r#"{"id":"28bf38e2","state":"failed"}"#, ThreadStatus::Failed)]
    #[case(r#"{"id":"28bf38e2","state":"stopped"}"#, ThreadStatus::Stopped)]
    #[case(r#"{"id":"28bf38e2","state":"done"}"#, ThreadStatus::Stopped)]
    #[case(r#"{"id":"28bf38e2","state":"blocked"}"#, ThreadStatus::Stopped)]
    #[case(r#"{"id":"28bf38e2"}"#, ThreadStatus::Stopped)]
    fn agent_record_maps_to_thread_status(
        #[case] record: &str,
        #[case] expected: ThreadStatus,
    ) -> Result<(), Report<SessionHostError>> {
        // Given one `claude agents --json` record.
        let json = format!("[{record}]");

        // When parsing it.
        let records = parse_agents(&json)?;

        // Then its status maps per the approved table.
        let statuses: Vec<ThreadStatus> = records.iter().map(|record| record.status).collect();
        assert_eq!(statuses, [expected], "{record} should map to {expected:?}");
        Ok(())
    }

    #[rstest::rstest]
    fn interactive_records_are_skipped() -> Result<(), Report<SessionHostError>> {
        // Given a background record and an interactive one, which has no id.
        let json = r#"[
          {"pid":960,"id":"28bf38e2","cwd":"/Users/felixpherry/dev/orb","kind":"background",
           "startedAt":1790233098717,"sessionId":"28bf38e2-8929-4841-b907-f87d5d469a10",
           "name":"orb-m2-probe","status":"idle","state":"blocked"},
          {"pid":63163,"kind":"interactive","startedAt":1790143927372,
           "name":"itemku-frontend-next-v2-18","status":"waiting","waitingFor":"dialog open"}
        ]"#;

        // When parsing them.
        let records = parse_agents(json)?;

        // Then only the background session is listed.
        let ids: Vec<&str> = records
            .iter()
            .map(|record| record.short_id.as_str())
            .collect();
        assert_eq!(
            ids,
            ["28bf38e2"],
            "the interactive record should be skipped"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn malformed_agents_json_is_an_error() {
        // Given output that isn't JSON.
        let text = "claude: command not found";

        // When parsing it.
        let result = parse_agents(text);

        // Then parsing fails.
        assert!(result.is_err(), "malformed JSON should be an error");
    }

    #[rstest::rstest]
    fn attach_argv_runs_claude_attach_with_the_short_id() {
        // Given a supervisor.
        let supervisor = ClaudeSupervisor::new(Vec::new());

        // When asking for the attach command.
        let argv = supervisor.attach_argv("28bf38e2");

        // Then it is `claude attach <id>`.
        assert_eq!(
            argv,
            ["claude", "attach", "28bf38e2"].map(OsString::from),
            "attach should run `claude attach 28bf38e2`"
        );
    }
}
