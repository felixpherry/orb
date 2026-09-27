//! PROTOTYPE: print every which-key variant for ␣, g and z on a fixture thread.
//! Args: `[width] [height]`.

#![expect(clippy::print_stdout, reason = "prints the variants")]

use std::time::SystemTime;

use orb_domain::AppState;
use orb_domain::feat::sessions::state::{
    Project, ProjectId, Sessions, SidebarItem, Thread, ThreadId, ThreadStatus,
};

fn main() {
    let mut args = std::env::args().skip(1).map(|arg| arg.parse().unwrap_or(0));
    let width = args.next().unwrap_or(140);
    let height = args.next().unwrap_or(36);
    let thread = Thread {
        id: ThreadId(1),
        title: Some("Sidebar rename, cursor and search".to_owned()),
        cwd: "/Users/me/.orb/worktrees/orb/orb-0cd0f215".into(),
        transcript: None,
        status: ThreadStatus::Working,
        turn_started_at: Some(SystemTime::now()),
        attach_argv: vec![],
        branch: Some("orb/sidebar-ui-refinement".to_owned()),
        pinned_at: None,
        settled_at: None,
        active_since: SystemTime::now(),
        last_activity_at: SystemTime::now(),
        unseen: false,
    };
    let mut state = AppState {
        sessions: Sessions {
            projects: vec![Project {
                id: ProjectId(1),
                title: "orb".to_owned(),
                root: "/Users/me/dev/orb".into(),
                created_at: SystemTime::now(),
                threads: vec![thread],
                draft: None,
                removed: false,
            }],
            cursor: Some(SidebarItem::Thread(ThreadId(1))),
            ..Sessions::default()
        },
        home: "/Users/me".into(),
        ..AppState::default()
    };
    println!(
        "{}",
        orb_tui::which_key_prototype::dump(&mut state, width, height)
    );
}
