//! Adding panes and saving layouts, the sidebar and the jump list.

use super::*;

impl SessionsActor {
    /// Adds a shell pane in `session`'s directory where `placement` says: in
    /// the shown tab re-tiled by pane count unless it was changed by hand,
    /// splitting the focused pane, in a new tab, or as a float. A session with no layout
    /// (its last pane closed) gets a first tab of that shell from
    /// [`Placement::Tab`] and is reported ready to open once it's saved; the other
    /// placements do nothing there. Then saves the layout. A pane that can't
    /// be saved shows why.
    pub(super) fn add_pane(&mut self, session: SessionId, placement: Placement) {
        let Some(dir) = self.sessions.get(&session).map(|row| row.dir.clone()) else {
            return;
        };
        let first = self.state.read().layouts.get(session).is_none();
        if first && !matches!(placement, Placement::Tab) {
            return;
        }
        let id = match self.store.insert_pane(session, &dir) {
            Ok(id) => id,
            Err(report) => {
                let error = save_failed(&self.services.log, &report);
                return self.report.report(Outcome::Failed(error));
            }
        };
        let row = PaneRow {
            id,
            session_id: session,
            cwd: dir,
            zmx_name: None,
            zmx_dir: None,
            resume: None,
            name: None,
        };
        let entry = pane_entry(&self.services, &self.orb_root, &row);
        {
            let mut app = self.state.write();
            match placement {
                Placement::Tile => app.layouts.add_tiled(session, entry),
                Placement::Split(split) => app.layouts.split(session, split, entry),
                Placement::Stack => app.layouts.stack(session, entry),
                Placement::Tab if first => app.layouts.insert(session, SessionLayout::of(entry)),
                Placement::Tab => app.layouts.new_tab(session, entry),
                Placement::Float => app.layouts.add_float(session, entry),
            }
        }
        self.save_layout(session);
        self.report.changed();
        if first {
            // Entering the session asked for this tab, so it is the active one.
            self.report.report(Outcome::Ready {
                session,
                if_active: Some(session),
            });
        }
    }

    /// Saves `session`'s tabs, floats, active tab and pane names as the app
    /// state has them, after them the tabs and panes orb couldn't read at
    /// start as they were saved; panes no tab or float holds any more are
    /// deleted from the store, each with its report file. A session whose layout emptied, or kept only floats, gets a
    /// new shell tab in its directory instead, saved in turn; nothing happens
    /// for one that is gone or being deleted.
    pub(super) fn save_layout(&mut self, session: SessionId) {
        let saved = {
            let app = self.state.read();
            app.layouts
                .get(session)
                .filter(|layout| !layout.tabs().is_empty())
                .map(|layout| {
                    let tabs: Vec<TabRow> = layout
                        .tabs()
                        .iter()
                        .enumerate()
                        .map(|(position, tab)| TabRow {
                            session_id: session,
                            position,
                            name: tab.name().map(str::to_owned),
                            layout: tab.layout_json(),
                            focus_pane: Some(tab.focused()),
                            swap_layout: tab.swap_layout(),
                            hand_changed: tab.hand_changed(),
                        })
                        .collect();
                    let names: Vec<(PaneId, Option<String>)> = layout
                        .panes()
                        .map(|entry| (entry.id, entry.name.clone()))
                        .collect();
                    let floats: Vec<FloatRow> = layout
                        .floats()
                        .all()
                        .iter()
                        .enumerate()
                        .map(|(position, float)| FloatRow {
                            session_id: session,
                            position,
                            pane_id: float.pane,
                            rect: float.rect,
                            pinned: float.pinned,
                        })
                        .collect();
                    let floats = SessionFloats {
                        rows: floats,
                        layout: layout.floats().layout(),
                        hand_changed: layout.floats().hand_changed(),
                    };
                    (tabs, floats, layout.active(), names)
                })
        };
        let Some((mut tabs, floats, active, mut names)) = saved else {
            let deleting = self.state.read().sessions.deleting.contains(&session);
            if self.sessions.contains_key(&session) && !deleting {
                self.add_pane(session, Placement::Tab);
            }
            return;
        };
        if let Some(unread) = self.unread.get(&session) {
            let read = tabs.len();
            tabs.extend(unread.tabs.iter().enumerate().map(|(i, tab)| TabRow {
                position: read + i,
                ..tab.clone()
            }));
            names.extend(
                unread
                    .panes
                    .iter()
                    .map(|entry| (entry.id, entry.name.clone())),
            );
        }
        match self
            .store
            .save_layout(session, active, &tabs, &floats, &names)
        {
            Ok(dropped) => {
                for pane in dropped {
                    self.services.log.line(
                        "delete",
                        &format!("pane row {} (session {})", pane.0, session.0),
                    );
                    for row in self.rows.iter_mut().filter(|row| row.pane_id == Some(pane)) {
                        row.pane_id = None;
                    }
                    self.forget_pane_report(pane);
                }
            }
            Err(report) => {
                let error = save_failed(&self.services.log, &report);
                self.report.report(Outcome::Failed(error));
            }
        }
    }

    pub(super) fn save_ui(&self) {
        let ui = {
            let app = self.state.read();
            Ui {
                sidebar_width: Some(app.sidebar.width),
                project_filter: app.sessions.filter,
            }
        };
        if let Err(report) = self.store.save_ui(&ui) {
            let error = save_failed(&self.services.log, &report);
            self.report.report(Outcome::Failed(error));
        }
    }

    /// Saves the jump list, showing why if it can't.
    pub(super) fn save_jumps(&self) {
        let jumps = self.state.read().jumps.entries().to_vec();
        if let Err(report) = self.store.save_jumps(&jumps) {
            let error = save_failed(&self.services.log, &report);
            self.report.report(Outcome::Failed(error));
        }
    }
}
