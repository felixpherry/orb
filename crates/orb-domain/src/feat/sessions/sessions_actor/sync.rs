//! The user's sync script, `~/.orb/sync`: run at start and every ten minutes
//! when it's an executable file. A failed run puts its last output lines on
//! the plate as one item, which later failures update and the next clean
//! run closes.

use std::os::unix::fs::PermissionsExt;
use std::process::Command;

use crate::common::{Finished, run_within};
use crate::feat::plate::{ItemState, NewPlateItem, PlateChange, Verb};

use super::*;

/// How often the sync script runs after the first, at start.
pub(super) const SYNC_EVERY: Duration = Duration::from_mins(10);
/// How long a run may take before it's killed and counted as failed.
const SYNC_LIMIT: Duration = Duration::from_mins(5);
/// How many of a failed run's last output lines the plate item shows.
const SYNC_LINES: usize = 5;
/// The title of the plate item a failed run puts on the plate.
pub(super) const SYNC_FAILED: &str = "Sync script failed";
/// The session the sync item names: none, as no session's id is 0.
const SYNC_SESSION: SessionId = SessionId(0);

/// How a run of the sync script ended.
#[derive(Debug, PartialEq, Eq)]
pub enum SyncRan {
    /// It exited 0.
    Passed,
    /// It failed; its last output lines, or why it couldn't run.
    Failed(String),
}

/// Runs `script` with no arguments, at most [`SYNC_LIMIT`]; `None` when it
/// isn't an executable file.
pub(super) fn run_sync(script: &Path) -> Option<SyncRan> {
    let executable = fs::metadata(script)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0);
    if !executable {
        return None;
    }
    let ran = match run_within(Command::new(script), SYNC_LIMIT) {
        Ok(Finished::Exited(output)) if output.status.success() => SyncRan::Passed,
        Ok(Finished::Exited(output)) => {
            let printed = [output.stdout, output.stderr].concat();
            let printed = String::from_utf8_lossy(&printed);
            let lines: Vec<&str> = printed
                .lines()
                .filter(|line| !line.trim().is_empty())
                .collect();
            let last = lines.len().saturating_sub(SYNC_LINES);
            match lines.get(last..) {
                Some(tail) if !tail.is_empty() => SyncRan::Failed(tail.join("\n")),
                _ => SyncRan::Failed(output.status.to_string()),
            }
        }
        Ok(Finished::TimedOut) => SyncRan::Failed(format!(
            "killed after {} minutes",
            SYNC_LIMIT.as_secs() / 60
        )),
        Err(error) => SyncRan::Failed(format!("couldn't run {}: {error}", script.display())),
    };
    Some(ran)
}

/// Runs `script` beside the actor and hands how it ended to `actor`; nothing
/// when it isn't an executable file. `false` once the actor has stopped.
pub(super) async fn sync(actor: &ActorRef<SessionsActor>, script: &Path) -> bool {
    let script = script.to_owned();
    match tokio::task::spawn_blocking(move || run_sync(&script)).await {
        Ok(Some(ran)) => actor.ask(ran).await.is_ok(),
        _ => true,
    }
}

impl Message<SyncRan> for SessionsActor {
    type Reply = ();

    fn handle(
        &mut self,
        msg: SyncRan,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> impl Future<Output = Self::Reply> + Send {
        self.synced(msg);
        std::future::ready(())
    }
}

impl SessionsActor {
    /// Puts a failed run on the plate, in the open sync item when there is
    /// one, and closes that item after a clean run.
    fn synced(&self, ran: SyncRan) {
        let open = self
            .state
            .read()
            .plate
            .items()
            .iter()
            .find(|item| {
                item.state.is_open() && item.session == SYNC_SESSION && item.title == SYNC_FAILED
            })
            .map(|item| item.id);
        let saved = match (ran, open) {
            (SyncRan::Passed, None) => Ok(()),
            (SyncRan::Passed, Some(item)) => self.finish_plate_item(&ApiPlateDone { item }),
            (SyncRan::Failed(what), Some(item)) => self.update_plate_item(&ApiPlateUpdate {
                item,
                change: PlateChange {
                    what: Some(what),
                    ..PlateChange::default()
                },
            }),
            (SyncRan::Failed(what), None) => self
                .add_plate_item(ApiPlateAdd {
                    item: NewPlateItem {
                        verb: Verb::Read,
                        state: ItemState::YourMove,
                        title: SYNC_FAILED.to_owned(),
                        what: Some(what),
                        links: Vec::new(),
                        urgent: false,
                        session: SYNC_SESSION,
                        session_title: "~/.orb/sync".to_owned(),
                        pane: PaneId(0),
                        harness: None,
                    },
                })
                .map(drop),
        };
        if let Err(report) = saved {
            self.services.log.error("sync script's plate item", &report);
        }
    }
}
