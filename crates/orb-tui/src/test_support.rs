//! Fixtures the frontend's tests share.

use orb_domain::feat::sessions::state::{Project, ProjectKind, Session, SessionKind};

/// The sessions `projects`' threads run in, one per pane session, as the
/// sessions actor would show them: each in its first thread's directory,
/// neither pinned nor settled, active since the thread's creation.
pub(crate) fn sessions_for(projects: &[Project]) -> Vec<Session> {
    let mut shown: Vec<Session> = Vec::new();
    for project in projects {
        for thread in &project.threads {
            let Some(id) = thread.session() else {
                continue;
            };
            if shown.iter().any(|session| session.id == id) {
                continue;
            }
            shown.push(Session {
                id,
                project: project.id,
                kind: match project.kind {
                    ProjectKind::Normal => SessionKind::Plain,
                    ProjectKind::Research => SessionKind::Research,
                    ProjectKind::Learn => SessionKind::Learn,
                    ProjectKind::Incognito => SessionKind::Incognito,
                },
                dir: thread.cwd.clone(),
                name: None,
                branch: None,
                created_at: thread.created_at,
                pinned_at: None,
                settled_at: None,
                active_since: thread.created_at,
                last_activity_at: thread.last_activity_at,
            });
        }
    }
    shown
}
