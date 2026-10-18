//! Opening the plate's links: a session's browser tab, links sent to the
//! browser already in one, and links handed to the desktop's opener.

use super::*;
use crate::feat::plate::links::browser_line;

/// A link for the opener, which runs outside orb's panes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum LinkJob {
    /// Open `url` as a new tab of the terminal-browser in pane `pane`.
    NewTab { url: String, pane: PaneId },
    /// Open `address` with the desktop's own opener.
    System(String),
}

/// The message box's words when a link can't open.
const LINK_FAILED: &str = "couldn't open the link";

impl SessionsActor {
    /// Opens a tab right after `session`'s shown tab, and shows it, whose
    /// fresh shell runs terminal-browser on `url`, and remembers its pane as
    /// the session's browser tab.
    pub(super) fn new_browser_tab(&mut self, session: SessionId, url: &str) {
        let line = Some(browser_line(url));
        if let Some(pane) = self.place_pane(session, Placement::TabAfterShown, line) {
            self.state.write().browser_tabs.remember(session, pane);
        }
    }

    /// Opens `url` as a new tab of the terminal-browser in pane `pane`,
    /// without waiting for it (see [`Self::open_link`]).
    pub(super) fn open_in_browser(&self, pane: PaneId, url: String) {
        drop(self.open_link(LinkJob::NewTab { url, pane }));
    }

    /// Opens `address` with the desktop's own opener, without waiting for
    /// it (see [`Self::open_link`]).
    pub(super) fn open_outside(&self, address: String) {
        drop(self.open_link(LinkJob::System(address)));
    }

    /// Hands `job` to the opener on a blocking task beside the actor, so the
    /// queue keeps moving while it runs. A link that fails to open is logged
    /// and shown in the message box. Nothing has to wait for the task.
    pub(super) fn open_link(&self, job: LinkJob) -> tokio::task::JoinHandle<()> {
        let links = self.services.links.clone();
        let log = self.services.log.clone();
        let report = self.report.clone();
        tokio::task::spawn_blocking(move || {
            let opened = match &job {
                LinkJob::NewTab { url, pane } => links.new_tab(url, *pane),
                LinkJob::System(address) => links.system_open(address),
            };
            if let Err(error) = opened {
                log.error("plate: open link", &error);
                report.report(Outcome::Failed(LINK_FAILED.to_owned()));
            }
        })
    }
}
