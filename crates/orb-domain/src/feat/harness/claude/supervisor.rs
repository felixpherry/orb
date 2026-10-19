//! How orb sees the Claudes users start in panes.
//!
//! Claude keeps a status file for each running Claude in
//! `<claude dir>/sessions/<pid>.json`. A file counts while its pid runs and
//! the start time it records matches that process's; one `ps` call gives the
//! start times and each process's parents, which tell the pane a Claude runs
//! in. A stopped process, as one paused with Ctrl-Z, counts as not running.
//! Only interactive Claudes count, not parked or spare ones.
//!
//! Claude rewrites a file in place, so a read can catch one half-written;
//! such a file keeps its last good reading. A sessions folder that can't be
//! read, or no status file reading on two polls in a row, is an error.
//! `ps` runs with orb's child environment and no stdin, and is killed if it
//! outlives its time limit.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use error_stack::{Report, ResultExt};
use serde::de::IgnoredAny;
use serde::{Deserialize, Deserializer};

use crate::common::{Finished, ancestry, run_within};
use crate::feat::harness::{HarnessError, RunningAgent};
use crate::feat::sessions::state::AgentStatus;

const LIST_TIMEOUT: Duration = Duration::from_secs(10);

/// Polls in a row on which no status file read before a poll fails.
const UNREAD_POLLS_LIMIT: u32 = 2;

/// `waitingFor` values that mean Claude waits for an approval.
const APPROVALS: [&str; 3] = ["permission prompt", "sandbox request", "worker request"];

/// The Claudes running now.
#[async_trait]
pub trait ClaudeAgents: Send + Sync {
    fn name(&self) -> &'static str;

    /// The interactive Claudes running now, each with its status and its
    /// process ancestry.
    ///
    /// # Errors
    ///
    /// Returns an error if Claude's sessions folder can't be read, or if
    /// none of its status files has read on two calls in a row.
    async fn list(&self) -> Result<Vec<RunningAgent>, Report<HarnessError>>;
}

/// Reads the status files Claude keeps in its sessions folder.
#[derive(Debug)]
pub struct ClaudeSupervisor {
    /// The environment `ps` runs with.
    env: Vec<(OsString, OsString)>,
    /// `<claude dir>/sessions`, holding one status file per running Claude.
    sessions: PathBuf,
    /// What earlier polls read.
    memory: Mutex<Memory>,
}

impl ClaudeSupervisor {
    /// A supervisor reading the status files under `claude_dir`, whose `ps`
    /// runs with exactly `env`.
    pub fn new(env: Vec<(OsString, OsString)>, claude_dir: &Path) -> Self {
        Self {
            env,
            sessions: claude_dir.join("sessions"),
            memory: Mutex::new(Memory::default()),
        }
    }

    /// `program` with `args` and orb's child environment.
    fn command(&self, program: &str, args: &[&str]) -> Command {
        let mut command = Command::new(program);
        command
            .args(args)
            .env_clear()
            .envs(self.env.iter().map(|(key, value)| (key, value)));
        command
    }

    /// `ps` listing every process's pid, parent, state and start time, the
    /// start time in UTC and the C locale, as status files record it.
    fn ps(&self) -> Command {
        let mut command = self.command("ps", &["-A", "-o", "pid=,ppid=,stat=,lstart="]);
        command.env("TZ", "UTC").env("LC_ALL", "C");
        command
    }

    fn memory(&self) -> MutexGuard<'_, Memory> {
        self.memory.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[async_trait]
impl ClaudeAgents for ClaudeSupervisor {
    fn name(&self) -> &'static str {
        "claude"
    }

    /// Reads the folder, `ps` and the status files on a blocking thread.
    async fn list(&self) -> Result<Vec<RunningAgent>, Report<HarnessError>> {
        let sessions = self.sessions.clone();
        let ps = self.ps();
        let poll = tokio::task::spawn_blocking(move || read_poll(&sessions, ps))
            .await
            .change_context(HarnessError)
            .attach("couldn't read Claude's status files".to_owned())??;
        let Some(Poll { files, processes }) = poll else {
            *self.memory() = Memory::default();
            return Ok(Vec::new());
        };
        self.memory()
            .select(&self.sessions, files, processes.as_ref())
    }
}

/// A status file's pid and what reading it gave.
type StatusFile = (u32, Result<StatusRecord, Report<HarnessError>>);

/// What one poll read.
struct Poll {
    /// The files of the Claudes still running.
    files: Vec<StatusFile>,
    /// `ps`'s answer, `None` when it failed.
    processes: Option<Processes>,
}

/// Lists `folder`, runs `ps` when it names any Claude, then reads the files
/// of the Claudes still running; `None` when it names none.
fn read_poll(folder: &Path, ps: Command) -> Result<Option<Poll>, Report<HarnessError>> {
    let pids = session_pids(folder)?;
    if pids.is_empty() {
        return Ok(None);
    }
    // ponytail: a failed `ps` leaves each agent its own pid, which
    // still matches a Claude that is its zmx session's program.
    let processes = match run_within(ps, LIST_TIMEOUT) {
        Ok(Finished::Exited(output)) if output.status.success() => {
            Some(parse_processes(&String::from_utf8_lossy(&output.stdout)))
        }
        _ => None,
    };
    let files = read_files(folder, &candidates(&pids, processes.as_ref()));
    Ok(Some(Poll { files, processes }))
}

/// The pids named by the status files in `folder`. A failure's reason is
/// three lines short enough for the message box: what failed, why, then the
/// folder.
fn session_pids(folder: &Path) -> Result<BTreeSet<u32>, Report<HarnessError>> {
    let entries = fs::read_dir(folder).map_err(|error| {
        let reason = format!(
            "can't read Claude's sessions folder\n{error}\n{}",
            folder.display()
        );
        Report::new(error)
            .change_context(HarnessError)
            .attach(reason)
    })?;
    Ok(entries
        .filter_map(Result::ok)
        .filter_map(|entry| pid_of(&entry.file_name()))
        .collect())
}

/// The pid a status file's name gives: ASCII digits, then `.json`.
fn pid_of(name: &OsStr) -> Option<u32> {
    let digits = name.to_str()?.strip_suffix(".json")?;
    if !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// What one `ps` call says about the running processes.
#[derive(Debug, Default)]
struct Processes {
    /// Each pid's parent.
    parents: HashMap<u32, u32>,
    /// Each pid's start time, its words joined by single spaces.
    started: HashMap<u32, String>,
    /// The pids that are stopped, as by Ctrl-Z.
    stopped: HashSet<u32>,
}

impl Processes {
    /// Whether `pid` started at `proc_start`; a file that records no start
    /// time matches any.
    fn started_at(&self, pid: u32, proc_start: Option<&str>) -> bool {
        proc_start.is_none_or(|proc_start| {
            self.started
                .get(&pid)
                .is_some_and(|started| started.split_whitespace().eq(proc_start.split_whitespace()))
        })
    }
}

/// `ps -o pid=,ppid=,stat=,lstart=` lines: pid, parent, state, then the
/// start time, which holds spaces and runs to the end of the line.
/// Unreadable lines are skipped.
fn parse_processes(stdout: &str) -> Processes {
    let mut processes = Processes::default();
    for line in stdout.lines() {
        let mut fields = line.split_whitespace();
        let (Some(pid), Some(parent), Some(state)) = (
            fields.next().and_then(|field| field.parse().ok()),
            fields.next().and_then(|field| field.parse().ok()),
            fields.next(),
        ) else {
            continue;
        };
        processes.parents.insert(pid, parent);
        if state.starts_with('T') {
            processes.stopped.insert(pid);
        }
        processes
            .started
            .insert(pid, fields.collect::<Vec<_>>().join(" "));
    }
    processes
}

/// The pids whose files are worth reading: those `ps` lists and doesn't show
/// stopped, or all of them when `ps` failed.
fn candidates(pids: &BTreeSet<u32>, processes: Option<&Processes>) -> Vec<u32> {
    pids.iter()
        .copied()
        .filter(|pid| {
            processes.is_none_or(|processes| {
                processes.parents.contains_key(pid) && !processes.stopped.contains(pid)
            })
        })
        .collect()
}

/// Each candidate's status file, read from `folder`.
fn read_files(folder: &Path, candidates: &[u32]) -> Vec<StatusFile> {
    candidates
        .iter()
        .map(|&pid| {
            let record = fs::read(folder.join(format!("{pid}.json")))
                .map_err(|error| file_failure(pid, error))
                .and_then(|bytes| parse_record(&bytes, pid));
            (pid, record)
        })
        .collect()
}

/// `pid`'s status file holding `bytes`, if it parses and names that pid. A
/// failure's reason starts with the file's name.
fn parse_record(bytes: &[u8], pid: u32) -> Result<StatusRecord, Report<HarnessError>> {
    let record: StatusRecord =
        serde_json::from_slice(bytes).map_err(|error| file_failure(pid, error))?;
    if record.pid == Some(pid) {
        Ok(record)
    } else {
        Err(Report::new(HarnessError).attach(format!("{pid}.json names pid {:?}", record.pid)))
    }
}

/// What earlier polls read.
#[derive(Debug, Default)]
struct Memory {
    /// Each pid's last status file that read, while its process still
    /// started when that reading says.
    last_good: BTreeMap<u32, StatusRecord>,
    /// Polls in a row on which no status file read.
    unread_polls: u32,
}

impl Memory {
    /// The running interactive Claudes in this poll's `files`, read from
    /// `folder`. A file that didn't read gives its last good reading, if its
    /// process still started when that reading says. Fails once no file has
    /// read on [`UNREAD_POLLS_LIMIT`] polls in a row, with a reason of three
    /// lines: what failed, the last file's failure, then the folder.
    fn select(
        &mut self,
        folder: &Path,
        files: Vec<StatusFile>,
        processes: Option<&Processes>,
    ) -> Result<Vec<RunningAgent>, Report<HarnessError>> {
        let read = files.iter().any(|(_, file)| file.is_ok());
        let mut failure = None;
        let mut last_good = BTreeMap::new();
        for (pid, file) in files {
            let record = match file {
                Ok(record) => Some(record),
                Err(report) => {
                    failure = Some(report);
                    self.last_good.remove(&pid)
                }
            };
            let current = record.filter(|record| {
                processes
                    .is_none_or(|processes| processes.started_at(pid, record.proc_start.as_deref()))
            });
            if let Some(record) = current {
                last_good.insert(pid, record);
            }
        }
        self.last_good = last_good;
        self.unread_polls = if read || failure.is_none() {
            0
        } else {
            self.unread_polls.saturating_add(1)
        };
        if let Some(failure) = failure.filter(|_| self.unread_polls >= UNREAD_POLLS_LIMIT) {
            let cause = failure.downcast_ref::<String>().map_or("", String::as_str);
            let reason = format!(
                "can't read Claude's status files\n{cause}\n{}",
                folder.display()
            );
            return Err(failure.attach(reason));
        }
        Ok(self
            .last_good
            .iter()
            .filter(|(_, record)| record.is_interactive())
            .map(|(&pid, record)| RunningAgent {
                status: status_of(record),
                ancestry: processes
                    .map_or_else(|| vec![pid], |processes| ancestry(pid, &processes.parents)),
            })
            .collect())
    }
}

/// A report on `pid`'s status file whose reason starts with the file's name.
fn file_failure(pid: u32, error: impl Error + Send + Sync + 'static) -> Report<HarnessError> {
    let reason = format!("{pid}.json: {error}");
    Report::new(error)
        .change_context(HarnessError)
        .attach(reason)
}

/// The fields orb reads from a Claude status file; others are ignored.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StatusRecord {
    pid: Option<u32>,
    kind: Option<String>,
    status: Option<String>,
    waiting_for: Option<String>,
    state: Option<String>,
    /// When the process started, as `ps -o lstart=` prints it in UTC.
    proc_start: Option<String>,
    /// Whether the file has a `parkedJobId` key, whatever its value.
    #[serde(default, rename = "parkedJobId", deserialize_with = "present")]
    parked: bool,
    spare: Option<bool>,
}

/// `true` for a key that is present, `null` included.
fn present<'de, D: Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    IgnoredAny::deserialize(deserializer).map(|_| true)
}

impl StatusRecord {
    /// A Claude a user runs in a pane: interactive, neither parked nor
    /// spare. A record's own `sessionId` is ignored; orb's hook says which
    /// conversation runs where.
    fn is_interactive(&self) -> bool {
        self.kind.as_deref() == Some("interactive") && self.spare != Some(true) && !self.parked
    }
}

fn status_of(record: &StatusRecord) -> AgentStatus {
    match (record.status.as_deref(), record.state.as_deref()) {
        (Some("busy" | "shell"), _) => AgentStatus::Working,
        (Some("waiting"), _) => match record.waiting_for.as_deref() {
            Some(waiting_for) if APPROVALS.contains(&waiting_for) => AgentStatus::NeedsApproval,
            _ => AgentStatus::NeedsInput,
        },
        (_, Some("failed")) => AgentStatus::Failed,
        (Some(_), _) => AgentStatus::Idle,
        (None, _) => AgentStatus::Stopped,
    }
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "tests propagate setup and poll failures with `?` and assert on the outcome"
)]
mod tests {
    use std::collections::{BTreeSet, HashMap};
    use std::ffi::OsStr;
    use std::fs;
    use std::path::Path;

    use error_stack::{Report, ResultExt};

    use crate::feat::sessions::pane_status::match_records;
    use crate::ids::PaneId;

    use super::{
        AgentStatus, ClaudeAgents, ClaudeSupervisor, Finished, HarnessError, LIST_TIMEOUT, Memory,
        Processes, RunningAgent, StatusFile as File, candidates, parse_processes, parse_record,
        pid_of, run_within,
    };

    const FOLDER: &str = "/claude/sessions";

    /// launchd, a shell under it, and a Claude under the shell started on a
    /// one-digit day.
    const PS: &str = "    1     0 Ss   Mon Oct 19 05:00:00 2026\n  100     1 S    Mon Oct 19 06:29:32 2026\n  200   100 S+   Mon Oct  6 06:29:32 2026\n";

    /// Pid `pid`'s status file holding `json`.
    fn file(pid: u32, json: &str) -> File {
        (pid, parse_record(json.as_bytes(), pid))
    }

    /// Pid `pid`'s status file caught half-written.
    fn torn(pid: u32) -> File {
        file(pid, r#"{"pid":"#)
    }

    fn poll(
        memory: &mut Memory,
        files: Vec<File>,
        processes: Option<&Processes>,
    ) -> Result<Vec<RunningAgent>, Report<HarnessError>> {
        memory.select(Path::new(FOLDER), files, processes)
    }

    fn statuses(agents: &[RunningAgent]) -> Vec<AgentStatus> {
        agents.iter().map(|agent| agent.status).collect()
    }

    /// The one-line reason a failed poll carries.
    fn reason(result: Result<Vec<RunningAgent>, Report<HarnessError>>) -> Option<String> {
        result.err()?.downcast_ref::<String>().cloned()
    }

    #[rstest::rstest]
    #[case(
        r#"{"pid":1,"kind":"interactive","status":"busy"}"#,
        AgentStatus::Working
    )]
    #[case(
        r#"{"pid":1,"kind":"interactive","status":"shell"}"#,
        AgentStatus::Working
    )]
    #[case(
        r#"{"pid":1,"kind":"interactive","status":"busy","state":"working"}"#,
        AgentStatus::Working
    )]
    #[case(
        r#"{"pid":1,"kind":"interactive","status":"waiting","waitingFor":"permission prompt"}"#,
        AgentStatus::NeedsApproval
    )]
    #[case(
        r#"{"pid":1,"kind":"interactive","status":"waiting","waitingFor":"sandbox request"}"#,
        AgentStatus::NeedsApproval
    )]
    #[case(
        r#"{"pid":1,"kind":"interactive","status":"waiting","waitingFor":"worker request"}"#,
        AgentStatus::NeedsApproval
    )]
    #[case(
        r#"{"pid":1,"kind":"interactive","status":"waiting","waitingFor":"input needed"}"#,
        AgentStatus::NeedsInput
    )]
    #[case(
        r#"{"pid":1,"kind":"interactive","status":"waiting","waitingFor":"dialog open"}"#,
        AgentStatus::NeedsInput
    )]
    #[case(
        r#"{"pid":1,"kind":"interactive","status":"waiting"}"#,
        AgentStatus::NeedsInput
    )]
    #[case(
        r#"{"pid":1,"kind":"interactive","status":"idle","state":"failed"}"#,
        AgentStatus::Failed
    )]
    #[case(
        r#"{"pid":1,"kind":"interactive","status":"idle","state":"blocked"}"#,
        AgentStatus::Idle
    )]
    #[case(
        r#"{"pid":1,"kind":"interactive","state":"failed"}"#,
        AgentStatus::Failed
    )]
    #[case(
        r#"{"pid":1,"kind":"interactive","state":"stopped"}"#,
        AgentStatus::Stopped
    )]
    #[case(
        r#"{"pid":1,"kind":"interactive","state":"done"}"#,
        AgentStatus::Stopped
    )]
    #[case(
        r#"{"pid":1,"kind":"interactive","state":"blocked"}"#,
        AgentStatus::Stopped
    )]
    #[case(r#"{"pid":1,"kind":"interactive"}"#, AgentStatus::Stopped)]
    #[case(r#"{"pid":1,"kind":"interactive","status":"idle"}"#, AgentStatus::Idle)]
    #[case(
        r#"{"pid":1,"kind":"interactive","status":"idle","spare":false}"#,
        AgentStatus::Idle
    )]
    fn status_file_maps_to_agent_status(
        #[case] record: &str,
        #[case] expected: AgentStatus,
    ) -> Result<(), Report<HarnessError>> {
        // Given one Claude's status file.
        let files = vec![file(1, record)];

        // When polling it.
        let agents = poll(&mut Memory::default(), files, None)?;

        // Then its status maps per the approved table.
        assert_eq!(
            statuses(&agents),
            [expected],
            "{record} should map to {expected:?}"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[case("123.json", Some(123))]
    #[case("123.4f2a.key", None)]
    #[case("abc.json", None)]
    #[case("12a.json", None)]
    #[case(".json", None)]
    #[case("+5.json", None)]
    #[case("123.json.tmp", None)]
    fn status_file_name_gives_its_pid(#[case] name: &str, #[case] expected: Option<u32>) {
        // Given a name in the sessions folder.
        let name = OsStr::new(name);

        // When reading the pid it names.
        let pid = pid_of(name);

        // Then only digits then `.json` name a pid.
        assert_eq!(pid, expected, "{name:?} should give {expected:?}");
    }

    #[rstest::rstest]
    #[case(r#"{"pid":1,"kind":"bg","status":"busy"}"#)]
    #[case(r#"{"pid":1,"kind":"daemon","status":"busy"}"#)]
    #[case(r#"{"pid":1,"kind":"daemon-worker","status":"busy"}"#)]
    #[case(r#"{"pid":1,"status":"busy"}"#)]
    #[case(r#"{"pid":1,"kind":"interactive","status":"busy","parkedJobId":"j1"}"#)]
    #[case(r#"{"pid":1,"kind":"interactive","status":"busy","parkedJobId":null}"#)]
    #[case(r#"{"pid":1,"kind":"interactive","status":"busy","parkedJobId":7}"#)]
    #[case(r#"{"pid":1,"kind":"interactive","status":"busy","spare":true}"#)]
    fn record_that_is_not_a_pane_claude_is_skipped(
        #[case] record: &str,
    ) -> Result<(), Report<HarnessError>> {
        // Given a status file of a Claude no user runs in a pane.
        let files = vec![file(1, record)];

        // When polling it.
        let agents = poll(&mut Memory::default(), files, None)?;

        // Then nothing is listed.
        assert!(agents.is_empty(), "{record} isn't a pane's agent");
        Ok(())
    }

    #[rstest::rstest]
    fn file_of_a_pid_that_isnt_running_is_not_read() {
        // Given files for a running pid and a gone one.
        let pids = BTreeSet::from([200, 300]);
        let processes = parse_processes(PS);

        // When picking the files to read.
        let picked = candidates(&pids, Some(&processes));

        // Then only the running pid's file is read.
        assert_eq!(picked, [200], "a gone Claude's file shouldn't be read");
    }

    #[rstest::rstest]
    fn every_file_is_read_when_ps_fails() {
        // Given files for two pids and no `ps` answer.
        let pids = BTreeSet::from([200, 300]);

        // When picking the files to read.
        let picked = candidates(&pids, None);

        // Then both are read.
        assert_eq!(picked, [200, 300], "without `ps` every file should be read");
    }

    #[rstest::rstest]
    #[case("T", false)]
    #[case("T+", false)]
    #[case("Ts", false)]
    #[case("S", true)]
    #[case("R+", true)]
    fn file_of_a_stopped_pid_is_not_read(#[case] state: &str, #[case] read: bool) {
        // Given a running pid's file and `ps` showing that pid in `state`.
        let pids = BTreeSet::from([200]);
        let processes =
            parse_processes(&format!("  200   100 {state}   Mon Oct 19 06:29:32 2026\n"));

        // When picking the files to read.
        let picked = candidates(&pids, Some(&processes));

        // Then the file is read only when the process isn't stopped.
        assert_eq!(
            !picked.is_empty(),
            read,
            "a process in state {state} should be read: {read}"
        );
    }

    #[rstest::rstest]
    fn paused_claude_beside_a_live_one_leaves_the_pane_the_live_ones_status()
    -> Result<(), Report<HarnessError>> {
        // Given a pane's shell 100 with a paused working Claude 200 and a live
        // idle Claude 300 under it.
        let processes = parse_processes(
            "  100     1 S    Mon Oct 19 05:00:00 2026\n  200   100 T    Mon Oct 19 06:00:00 2026\n  300   100 S+   Mon Oct 19 06:29:32 2026\n",
        );
        let records = HashMap::from([
            (200, r#"{"pid":200,"kind":"interactive","status":"busy"}"#),
            (300, r#"{"pid":300,"kind":"interactive","status":"idle"}"#),
        ]);
        let pids: BTreeSet<u32> = records.keys().copied().collect();

        // When polling and matching the Claudes to the pane.
        let files = candidates(&pids, Some(&processes))
            .into_iter()
            .filter_map(|pid| records.get(&pid).map(|json| file(pid, json)))
            .collect();
        let agents = poll(&mut Memory::default(), files, Some(&processes))?;
        let panes = match_records(&agents, &HashMap::from([(100, PaneId(7))]));

        // Then the pane takes the live Claude's status.
        assert_eq!(
            panes,
            HashMap::from([(PaneId(7), AgentStatus::Idle)]),
            "a paused Claude shouldn't speak for its pane"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn file_with_another_start_time_is_skipped() -> Result<(), Report<HarnessError>> {
        // Given a file whose pid now belongs to a process started later.
        let files = vec![file(
            200,
            r#"{"pid":200,"kind":"interactive","status":"busy","procStart":"Mon Oct  6 05:00:00 2026"}"#,
        )];
        let processes = parse_processes(PS);

        // When polling it.
        let agents = poll(&mut Memory::default(), files, Some(&processes))?;

        // Then nothing is listed.
        assert!(
            agents.is_empty(),
            "a reused pid's old file should be skipped"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn file_with_the_same_start_time_spaced_otherwise_is_kept() -> Result<(), Report<HarnessError>>
    {
        // Given a file whose start time pads the day with one space less than `ps`.
        let files = vec![file(
            200,
            r#"{"pid":200,"kind":"interactive","status":"busy","procStart":"Mon Oct 6 06:29:32 2026"}"#,
        )];
        let processes = parse_processes(PS);

        // When polling it.
        let agents = poll(&mut Memory::default(), files, Some(&processes))?;

        // Then the Claude is listed.
        assert_eq!(
            statuses(&agents),
            [AgentStatus::Working],
            "start times should compare word by word"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn file_without_a_start_time_is_kept() -> Result<(), Report<HarnessError>> {
        // Given a running pid's file that records no start time.
        let files = vec![file(
            200,
            r#"{"pid":200,"kind":"interactive","status":"busy"}"#,
        )];
        let processes = parse_processes(PS);

        // When polling it.
        let agents = poll(&mut Memory::default(), files, Some(&processes))?;

        // Then the Claude is listed.
        assert_eq!(
            statuses(&agents),
            [AgentStatus::Working],
            "a file with no start time should count by its pid"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn torn_file_after_a_good_read_keeps_the_earlier_status() -> Result<(), Report<HarnessError>> {
        // Given a Claude whose file read as working.
        let mut memory = Memory::default();
        poll(
            &mut memory,
            vec![file(1, r#"{"pid":1,"kind":"interactive","status":"busy"}"#)],
            None,
        )?;

        // When the next poll catches its file half-written.
        let agents = poll(&mut memory, vec![torn(1)], None)?;

        // Then it still reads working.
        assert_eq!(
            statuses(&agents),
            [AgentStatus::Working],
            "a torn file should keep its last good reading"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn torn_file_of_a_restarted_pid_is_skipped() -> Result<(), Report<HarnessError>> {
        // Given a Claude whose file read as working before its pid was reused.
        let mut memory = Memory::default();
        poll(
            &mut memory,
            vec![file(
                200,
                r#"{"pid":200,"kind":"interactive","status":"busy","procStart":"Mon Oct 6 05:00:00 2026"}"#,
            )],
            None,
        )?;
        let processes = parse_processes(PS);

        // When the next poll catches its file half-written.
        let agents = poll(&mut memory, vec![torn(200)], Some(&processes))?;

        // Then the old reading isn't used.
        assert!(
            agents.is_empty(),
            "a reading from another process shouldn't stand in for a torn file"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn new_torn_file_is_skipped() -> Result<(), Report<HarnessError>> {
        // Given a Claude whose file has never read.
        let files = vec![torn(1)];

        // When polling it.
        let agents = poll(&mut Memory::default(), files, None)?;

        // Then nothing is listed, and the poll succeeds.
        assert!(agents.is_empty(), "a new torn file should be skipped");
        Ok(())
    }

    #[rstest::rstest]
    fn file_naming_another_pid_is_skipped() -> Result<(), Report<HarnessError>> {
        // Given a file whose name and content disagree on the pid.
        let files = vec![file(5, r#"{"pid":6,"kind":"interactive","status":"busy"}"#)];

        // When polling it.
        let agents = poll(&mut Memory::default(), files, None)?;

        // Then nothing is listed.
        assert!(agents.is_empty(), "a file for another pid shouldn't read");
        Ok(())
    }

    #[rstest::rstest]
    fn no_file_reading_on_two_polls_is_an_error_giving_the_cause_then_the_folder() {
        // Given a poll on which no file read.
        let mut memory = Memory::default();
        let first = poll(&mut memory, vec![torn(1), torn(2)], None);

        // When the next poll reads none either.
        let second = poll(&mut memory, vec![torn(1), torn(2)], None);

        // Then it fails with what failed, the last file's failure and the
        // folder, each on its own line.
        assert_eq!(
            (first.is_ok(), reason(second)),
            (
                true,
                Some(
                    "can't read Claude's status files\n2.json: EOF while parsing a value at line 1 column 7\n/claude/sessions"
                        .to_owned()
                )
            ),
            "the second poll with nothing read should fail and give the cause on its own line"
        );
    }

    #[rstest::rstest]
    fn one_good_file_resets_the_unread_count() -> Result<(), Report<HarnessError>> {
        // Given a poll on which no file read, then one where one file read.
        let mut memory = Memory::default();
        poll(&mut memory, vec![torn(1)], None)?;
        poll(
            &mut memory,
            vec![
                torn(1),
                file(2, r#"{"pid":2,"kind":"interactive","status":"idle"}"#),
            ],
            None,
        )?;

        // When the next poll reads nothing.
        let third = poll(&mut memory, vec![torn(1)], None);

        // Then it succeeds.
        assert!(
            third.is_ok(),
            "a poll that read a file should restart the count"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn ancestry_without_ps_is_the_claude_alone() -> Result<(), Report<HarnessError>> {
        // Given a file and no `ps` answer.
        let files = vec![file(
            200,
            r#"{"pid":200,"kind":"interactive","status":"busy"}"#,
        )];

        // When polling it.
        let agents = poll(&mut Memory::default(), files, None)?;

        // Then its ancestry is its own pid.
        assert_eq!(
            agents,
            [RunningAgent {
                status: AgentStatus::Working,
                ancestry: vec![200],
            }],
            "a failed `ps` should leave each Claude its own pid"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn ancestry_with_ps_runs_up_to_launchd() -> Result<(), Report<HarnessError>> {
        // Given a file and the processes above it.
        let files = vec![file(
            200,
            r#"{"pid":200,"kind":"interactive","status":"busy"}"#,
        )];
        let processes = parse_processes(PS);

        // When polling it.
        let agents = poll(&mut Memory::default(), files, Some(&processes))?;

        // Then its ancestry walks its parents.
        assert_eq!(
            agents,
            [RunningAgent {
                status: AgentStatus::Working,
                ancestry: vec![200, 100, 1],
            }],
            "the ancestry should tell the pane the Claude runs in"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn ps_start_time_runs_to_the_end_of_the_line() {
        // Given `ps` lines with the state before the start time.
        let stdout = "  200   100 S+   Mon Oct  6 06:29:32 2026\nnot a process\n";

        // When parsing them.
        let processes = parse_processes(stdout);

        // Then the start time is every word after the state.
        assert!(
            processes.started_at(200, Some("Mon Oct 6 06:29:32 2026")),
            "the start time should be the rest of the line"
        );
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn missing_sessions_folder_is_an_error_giving_the_cause_then_the_folder()
    -> Result<(), Report<HarnessError>> {
        // Given a Claude folder with no sessions folder.
        let claude_dir = tempfile::tempdir().change_context(HarnessError)?;
        let supervisor = ClaudeSupervisor::new(Vec::new(), claude_dir.path());

        // When listing the Claudes.
        let result = supervisor.list().await;

        // Then the reason gives what failed, why, and the folder, each on its
        // own line.
        let folder = claude_dir.path().join("sessions");
        let cause = fs::read_dir(&folder)
            .err()
            .map(|error| error.to_string())
            .unwrap_or_default();
        assert_eq!(
            reason(result),
            Some(format!(
                "can't read Claude's sessions folder\n{cause}\n{}",
                folder.display()
            )),
            "a missing sessions folder should fail and give the cause on its own line"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn sessions_folder_without_status_files_lists_nothing() -> Result<(), Report<HarnessError>>
    {
        // Given a sessions folder holding only a key file.
        let claude_dir = tempfile::tempdir().change_context(HarnessError)?;
        let sessions = claude_dir.path().join("sessions");
        fs::create_dir_all(&sessions).change_context(HarnessError)?;
        fs::write(sessions.join("123.4f2a.key"), "key").change_context(HarnessError)?;
        let supervisor = ClaudeSupervisor::new(Vec::new(), claude_dir.path());

        // When listing the Claudes.
        let agents = supervisor.list().await?;

        // Then none are listed.
        assert!(agents.is_empty(), "a key file isn't a status file");
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn status_file_of_a_running_claude_is_listed() -> Result<(), Report<HarnessError>> {
        // Given a status file for this test's own process.
        let claude_dir = tempfile::tempdir().change_context(HarnessError)?;
        let sessions = claude_dir.path().join("sessions");
        fs::create_dir_all(&sessions).change_context(HarnessError)?;
        let pid = std::process::id();
        fs::write(
            sessions.join(format!("{pid}.json")),
            format!(r#"{{"pid":{pid},"kind":"interactive","status":"busy"}}"#),
        )
        .change_context(HarnessError)?;
        let supervisor = ClaudeSupervisor::new(Vec::new(), claude_dir.path());

        // When listing the Claudes.
        let agents = supervisor.list().await?;

        // Then it is listed as working, from its own pid up.
        let listed: Vec<_> = agents
            .iter()
            .map(|agent| (agent.status, agent.ancestry.first().copied()))
            .collect();
        assert_eq!(
            listed,
            [(AgentStatus::Working, Some(pid))],
            "a running process's status file should be listed"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn status_file_with_its_real_start_time_is_listed_under_its_parents()
    -> Result<(), Report<HarnessError>> {
        // Given a status file for this test's own process that records the
        // start time `ps` gives it in UTC.
        let claude_dir = tempfile::tempdir().change_context(HarnessError)?;
        let sessions = claude_dir.path().join("sessions");
        fs::create_dir_all(&sessions).change_context(HarnessError)?;
        let env = vec![("PATH".into(), std::env::var_os("PATH").unwrap_or_default())];
        let supervisor = ClaudeSupervisor::new(env, claude_dir.path());
        let pid = std::process::id();
        let mut ps = supervisor.command("ps", &["-o", "lstart=", "-p", &pid.to_string()]);
        ps.env("TZ", "UTC").env("LC_ALL", "C");
        let Finished::Exited(output) = run_within(ps, LIST_TIMEOUT).change_context(HarnessError)?
        else {
            return Err(Report::new(HarnessError).attach("ps timed out"));
        };
        let started = String::from_utf8_lossy(&output.stdout);
        let record = serde_json::json!({
            "pid": pid,
            "kind": "interactive",
            "status": "busy",
            "procStart": started.trim(),
        });
        fs::write(sessions.join(format!("{pid}.json")), record.to_string())
            .change_context(HarnessError)?;

        // When listing the Claudes.
        let agents = supervisor.list().await?;

        // Then it is listed as working, under its parent.
        let listed: Vec<_> = agents
            .iter()
            .map(|agent| (agent.status, agent.ancestry.get(..2).map(<[u32]>::to_vec)))
            .collect();
        assert_eq!(
            listed,
            [(
                AgentStatus::Working,
                Some(vec![pid, std::os::unix::process::parent_id()])
            )],
            "a status file whose start time matches `ps` should be listed under its parents"
        );
        Ok(())
    }
}
