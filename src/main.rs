//! Binary entry point for orb. This is the only place that reads process state
//! (the environment).

use std::path::PathBuf;
use std::sync::Arc;

use error_stack::{Report, ResultExt};
use jiff::tz::TimeZone;
use orb_domain::feat::git::git_cli::GitCli;
use orb_domain::feat::git::git_service::GitService;
use orb_domain::feat::notify::notifier::NotifierService;
use orb_domain::feat::notify::terminal_notifier::{ClickTarget, Kitty, ZellijTarget, on_path};
use orb_domain::feat::sessions::child_env::child_env;
use orb_domain::feat::sessions::claude_supervisor::ClaudeSupervisor;
use orb_domain::feat::sessions::session_host::SessionHostService;
use orb_domain::feat::sessions::sessions_actor::{SessionsActorDeps, spawn_sessions_actor};
use orb_domain::feat::sessions::store::Store;
use orb_domain::feat::zellij::zellij_cli::ZellijCli;
use orb_domain::feat::zellij::zellij_service::ZellijService;
use orb_domain::{AppState, Services, State};
use orb_tui::Frontend;
use wherror::Error;

/// orb couldn't start, or its frontend failed.
#[derive(Debug, Error)]
#[error(debug)]
struct OrbError;

fn main() -> Result<(), Report<OrbError>> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| Report::new(OrbError).attach("HOME is not set"))?;
    let claude_dir =
        std::env::var_os("CLAUDE_CONFIG_DIR").map_or_else(|| home.join(".claude"), PathBuf::from);
    let claude_env = child_env(std::env::vars_os());
    let tz = TimeZone::system();
    let session = std::env::var_os("ZELLIJ_SESSION_NAME");
    let pane = std::env::var("ZELLIJ_PANE_ID")
        .ok()
        .and_then(|id| id.parse().ok());
    let notifier = {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let kitty = {
            let kitten = std::env::var_os("KITTY_INSTALLATION_DIR")
                .map(|dir| PathBuf::from(dir).join("../../MacOS/kitten"))
                .filter(|kitten| kitten.is_file())
                .or_else(|| on_path("kitten", &path));
            let socket = std::env::var("KITTY_LISTEN_ON").ok();
            let window = std::env::var("KITTY_WINDOW_ID")
                .ok()
                .and_then(|id| id.parse().ok());
            kitten.zip(socket).map(|(kitten, socket)| Kitty {
                kitten,
                socket,
                window,
            })
        };
        let zellij = {
            let session = session.clone().and_then(|name| name.into_string().ok());
            let zellij = on_path("zellij", &path);
            match (zellij, session, pane) {
                (Some(zellij), Some(session), Some(pane)) => Some(ZellijTarget {
                    zellij,
                    session,
                    pane,
                }),
                _ => None,
            }
        };
        NotifierService::desktop(&path, ClickTarget { kitty, zellij })
    };
    let zellij = session.map(|_| {
        let shell = std::env::var_os("SHELL").unwrap_or_else(|| "sh".into());
        let cli = ZellijCli::new(std::env::var_os("NO_COLOR"));
        ZellijService::new(Arc::new(cli), shell, home.clone(), pane)
    });
    let store = Store::open(&home.join(".orb/userdata/state.sqlite")).change_context(OrbError)?;
    let worktrees_root = home.join(".orb/worktrees");
    let runtime = tokio::runtime::Runtime::new().change_context(OrbError)?;
    let _context = runtime.enter();
    let state = State::new(AppState {
        home,
        ..AppState::default()
    });
    let frontend = Frontend::new(tz);
    let services = Services {
        session_host: SessionHostService::new(Arc::new(ClaudeSupervisor::new(claude_env.clone()))),
        git: GitService::new(Arc::new(GitCli::new(claude_env.clone()))),
    };
    let git = services.git.clone();
    let sessions = spawn_sessions_actor(SessionsActorDeps {
        services,
        state: state.clone(),
        store,
        claude_dir,
        worktrees_root,
        wake: frontend.waker(),
    });
    frontend
        .run(state, sessions, git, claude_env, zellij, notifier)
        .change_context(OrbError)
}
