//! Binary entry point for orb. This is the only place that reads process state
//! (the environment).

use std::path::PathBuf;
use std::sync::Arc;

use error_stack::{Report, ResultExt};
use orb_domain::feat::git::git_cli::GitCli;
use orb_domain::feat::git::git_service::GitService;
use orb_domain::feat::preview::preview_actor::{PreviewActorDeps, spawn_preview_actor};
use orb_domain::feat::sessions::child_env::child_env;
use orb_domain::feat::sessions::claude_supervisor::ClaudeSupervisor;
use orb_domain::feat::sessions::session_host::SessionHostService;
use orb_domain::feat::sessions::sessions_actor::{SessionsActorDeps, spawn_sessions_actor};
use orb_domain::feat::sessions::store::Store;
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
    let store = Store::open(&home.join(".orb/userdata/state.sqlite")).change_context(OrbError)?;
    let runtime = tokio::runtime::Runtime::new().change_context(OrbError)?;
    let _context = runtime.enter();
    let state = State::new(AppState {
        home,
        ..AppState::default()
    });
    let frontend = Frontend::default();
    let services = Services {
        session_host: SessionHostService::new(Arc::new(ClaudeSupervisor::new(claude_env.clone()))),
        git: GitService::new(Arc::new(GitCli::new(claude_env.clone()))),
    };
    let sessions = spawn_sessions_actor(SessionsActorDeps {
        services,
        state: state.clone(),
        store,
        claude_dir,
        wake: frontend.waker(),
    });
    let preview = spawn_preview_actor(PreviewActorDeps {
        state: state.clone(),
        wake: frontend.waker(),
    });
    frontend
        .run(state, sessions, preview, claude_env)
        .change_context(OrbError)
}
