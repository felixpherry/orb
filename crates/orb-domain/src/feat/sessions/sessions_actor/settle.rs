//! Pinning, renaming, settling and deleting sessions, and killing their panes.

use super::*;

impl SessionsActor {
    /// Pins session `id`; pinning a settled session un-settles it.
    pub(super) fn pin_session(&mut self, id: SessionId) {
        self.edit_session(id, |row, now| {
            row.pinned_at = row.pinned_at.or(Some(now));
            if row.settled_override == Some(SettledOverride::Settled) {
                unsettle_session_row(row, now);
            }
        });
    }

    pub(super) fn unpin_session(&mut self, id: SessionId) {
        self.edit_session(id, |row, _| row.pinned_at = None);
    }

    /// Gives session `id` the user's name; `None` goes back to its agents'
    /// or directory's.
    pub(super) fn rename_session(&mut self, id: SessionId, name: Option<String>) {
        self.edit_session(id, |row, _| row.name = name);
    }

    /// Session `id`'s panes: the ones its layout holds and the ones its
    /// unread tabs keep.
    pub(super) fn session_pane_ids(&self, id: SessionId) -> HashSet<PaneId> {
        let app = self.state.read();
        session_entries(&app.layouts, &self.unread, id)
            .into_iter()
            .map(|entry| entry.id)
            .collect()
    }

    /// The saved agents running in session `id`'s panes.
    pub(super) fn session_rows(&self, id: SessionId) -> impl Iterator<Item = &AgentRow> {
        let panes = self.session_pane_ids(id);
        self.rows
            .iter()
            .filter(move |row| row.pane_id.is_some_and(|pane| panes.contains(&pane)))
    }

    /// Settles session `id` unless one of its agent panes has a turn
    /// underway (a turn that started after the key press wins), then kills
    /// every pane, keeping its tabs, layout and each pane's resume command.
    pub(super) fn settle_session(&mut self, id: SessionId) {
        if self
            .session_rows(id)
            .any(|row| self.status(row.id).is_some_and(AgentStatus::in_progress))
        {
            return;
        }
        self.edit_session(id, settle_session_row);
        self.kill_panes(id, "session settled");
    }

    pub(super) fn unsettle_session(&mut self, id: SessionId) {
        self.edit_session(id, unsettle_session_row);
    }

    /// Marks the latest turn of the agent in pane `pane` seen.
    pub(super) fn visit(&mut self, pane: PaneId) {
        let unseen = self
            .rows
            .iter()
            .find(|row| row.pane_id == Some(pane) && row.last_activity_at > row.last_visited_at)
            .map(|row| row.id);
        if let Some(agent) = unseen {
            self.edit(agent, |row, now| row.last_visited_at = now);
        }
    }

    /// Deletes pane `pane`'s report file (one already gone is fine; any other
    /// failure is logged) and forgets its latest report.
    pub(super) fn forget_pane_report(&mut self, pane: PaneId) {
        if let Err(error) = fs::remove_file(self.pane_files.file(pane))
            && error.kind() != io::ErrorKind::NotFound
        {
            self.services.log.error(
                &format!("couldn't remove pane {}'s report file", pane.0),
                &error,
            );
        }
        self.pane_reports.remove(&pane);
    }

    /// Deletes session `id`: kills every pane, deletes a Research or Learn
    /// folder that is orb's own and no other session uses, then its tabs,
    /// panes, agents (ended ones included) and row, and forgets them.
    /// Claude transcripts, pi session files and worktrees stay. However it
    /// ends, the session is no longer hidden as being deleted.
    pub(super) fn delete_session(&mut self, id: SessionId) {
        self.kill_panes(id, "session deleted");
        let cleared = self.clear_folder(id);
        let agents: Vec<AgentId> = self
            .session_rows(id)
            .chain(self.rows.iter().filter(|row| row.orb_session == Some(id)))
            .map(|row| row.id)
            .collect();
        let panes: Vec<PaneId> = {
            let app = self.state.read();
            session_entries(&app.layouts, &self.unread, id)
                .into_iter()
                .map(|entry| entry.id)
                .collect()
        };
        let deleted = self.store.delete_session(id);
        {
            let what = format!(
                "session {} with agents {:?} and panes {:?}",
                id.0,
                sorted_ids(agents.iter().map(|agent| agent.0)),
                sorted_ids(panes.iter().map(|pane| pane.0))
            );
            match &deleted {
                Ok(()) => self.services.log.line("delete", &format!("{what}: ok")),
                Err(report) => self
                    .services
                    .log
                    .error(&format!("delete {what}: failed"), report),
            }
        }
        self.rows.retain(|row| !agents.contains(&row.id));
        for pane in panes {
            self.forget_pane_report(pane);
        }
        self.sessions.remove(&id);
        self.unread.remove(&id);
        {
            let mut app = self.state.write();
            app.layouts.remove(id);
            app.attached.remove(&id);
            let sessions = &mut app.sessions;
            sessions.sessions.retain(|session| session.id != id);
            for project in &mut sessions.projects {
                project.agents.retain(|agent| !agents.contains(&agent.id));
            }
            sessions.deleting.remove(&id);
        }
        match (deleted, cleared) {
            (Err(report), _) => {
                let error = save_failed(&self.services.log, &report);
                self.report.report(Outcome::Failed(error));
            }
            (Ok(()), Err(error)) => self.report.report(Outcome::Failed(error)),
            (Ok(()), Ok(())) => self.report.changed(),
        }
    }

    /// Kills the panes zmx still lists for every settled session, listing
    /// each socket dir once, in pane order; each session with a pane to kill
    /// leaves the right side first. A dir zmx can't list kills nothing.
    pub(super) fn kill_settled(&self) {
        let zmx: Vec<(SessionId, ZmxSession)> = {
            let app = self.state.read();
            let mut entries: Vec<(SessionId, PaneEntry)> = self
                .sessions
                .values()
                .filter(|session| session.settled_override == Some(SettledOverride::Settled))
                .flat_map(|session| {
                    session_entries(&app.layouts, &self.unread, session.id)
                        .into_iter()
                        .map(|entry| (session.id, entry))
                })
                .collect();
            entries.sort_by_key(|(_, entry)| entry.id.0);
            entries
                .into_iter()
                .map(|(session, entry)| (session, entry.zmx))
                .collect()
        };
        let listed: HashMap<&Path, Vec<ZmxEntry>> = zmx
            .iter()
            .map(|(_, session)| session.dir.as_path())
            .collect::<HashSet<&Path>>()
            .into_iter()
            .map(|dir| {
                let listed = self
                    .services
                    .zmx
                    .list(dir)
                    .inspect_err(|report| {
                        self.services.log.error(
                            &format!("couldn't list zmx sessions on {}", dir.display()),
                            report,
                        );
                    })
                    .unwrap_or_default();
                (dir, listed)
            })
            .collect();
        let running: Vec<&(SessionId, ZmxSession)> = zmx
            .iter()
            .filter(|(_, session)| {
                listed
                    .get(session.dir.as_path())
                    .is_some_and(|entries| entries.iter().any(|entry| entry.name == session.name))
            })
            .collect();
        self.state
            .write()
            .attached
            .retain(|shown| !running.iter().any(|(owner, _)| owner == shown));
        for (_, session) in running {
            self.services.zmx.kill(session, "session settled");
        }
    }

    /// Takes session `id` off the right side, then runs `zmx kill` on each
    /// of its panes in pane order; each kill is logged with `why`. The state
    /// lock is released before the first kill.
    pub(super) fn kill_panes(&self, id: SessionId, why: &str) {
        let zmx: Vec<ZmxSession> = {
            let mut app = self.state.write();
            app.attached.remove(&id);
            session_entries(&app.layouts, &self.unread, id)
                .into_iter()
                .map(|entry| entry.zmx)
                .collect()
        };
        for session in &zmx {
            self.services.zmx.kill(session, why);
        }
    }

    /// Removes session `id`'s folder when it is a Research or Learn session
    /// in orb's own folder for its kind and no other session uses it
    /// (already gone counts as done). The error is the message box text.
    fn clear_folder(&self, id: SessionId) -> Result<(), String> {
        let Some(row) = self.sessions.get(&id) else {
            return Ok(());
        };
        let kind_dir = match row.kind {
            SessionKind::Research => "research",
            SessionKind::Learn => "learn",
            SessionKind::Plain | SessionKind::Incognito => return Ok(()),
        };
        let shared = self
            .sessions
            .values()
            .any(|other| other.id != id && other.dir == row.dir);
        if shared || row.dir.parent() != Some(self.orb_root.join(kind_dir).as_path()) {
            return Ok(());
        }
        match fs::remove_dir_all(&row.dir) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(failed(&self.services.log, FOLDER_UNREMOVED, &error)),
        }
    }
}

/// Settles session `row` onto the shelf as of `at`, unpinning it.
pub(super) fn settle_session_row(row: &mut SessionRow, at: i64) {
    row.settled_override = Some(SettledOverride::Settled);
    row.settled_at = Some(at);
    row.unsettled_at = None;
    row.pinned_at = None;
}

/// Un-settles session `row` and keeps it active until its next turn
/// activity. It re-enters Active at `now` unless it was already kept active.
fn unsettle_session_row(row: &mut SessionRow, now: i64) {
    if row.settled_override != Some(SettledOverride::Active) {
        row.unsettled_at = Some(now);
    }
    row.settled_override = Some(SettledOverride::Active);
    row.settled_at = None;
}
