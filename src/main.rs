//! Binary entry point for orb. This is the only place that reads process state
//! (the environment).

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use error_stack::{Report, ResultExt};
use jiff::tz::TimeZone;
use orb_domain::feat::git::git_cli::GitCli;
use orb_domain::feat::git::git_service::GitService;
use orb_domain::feat::harness::Harnesses;
use orb_domain::feat::harness::claude::ClaudeCode;
use orb_domain::feat::harness::claude::supervisor::ClaudeSupervisor;
use orb_domain::feat::harness::pi::Pi;
use orb_domain::feat::integration::{self, IntegrationPaths};
use orb_domain::feat::notify::click::{ClickTarget, Kitty, NiriTarget, on_path};
use orb_domain::feat::notify::notifier::NotifierService;
use orb_domain::feat::search::search_actor::{SearchActorDeps, spawn_search_actor};
use orb_domain::feat::sessions::child_env::child_env;
use orb_domain::feat::sessions::sessions_actor::{
    SessionsActorDeps, StopMigrated, spawn_sessions_actor,
};
use orb_domain::feat::sessions::store::Store;
use orb_domain::feat::worktrees::worktrees_actor::{
    SWEEP_EVERY, WorktreesActorDeps, spawn_worktrees_actor,
};
use orb_domain::feat::zmx::zmx_cli::ZmxCli;
use orb_domain::feat::zmx::zmx_service::ZmxService;
use orb_domain::{AppState, Finished, Services, State, ancestry, parse_parents, run_within};
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
    let config_dir = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from);
    let claude_dir = integration::claude_dir(config_dir, &home);
    let orb_root = home.join(".orb");
    let integrations = integration_paths(&home, &orb_root, &claude_dir);
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "integration") {
        return install_integrations(&args, &integrations, &home);
    }
    let pi_sessions = std::env::var_os("PI_CODING_AGENT_SESSION_DIR")
        .map_or_else(|| home.join(".pi/agent/sessions"), PathBuf::from);
    let env = child_env(std::env::vars_os());
    let tz = TimeZone::system();
    let notifier = desktop_notifier();
    let store = Store::open(&orb_root.join("userdata/state.sqlite")).change_context(OrbError)?;
    let search_index = orb_root.join("userdata/search.sqlite");
    let worktrees_root = orb_root.join("worktrees");
    let runtime = tokio::runtime::Runtime::new().change_context(OrbError)?;
    let _context = runtime.enter();
    let state = State::new(AppState {
        home,
        ..AppState::default()
    });
    let frontend = Frontend::new(tz);
    let services = {
        let git = GitService::new(Arc::new(GitCli::new(env.clone())));
        let claude = ClaudeCode::new(Arc::new(ClaudeSupervisor::new(env.clone())), claude_dir);
        let pi = Pi::new(pi_sessions);
        Services {
            harnesses: Harnesses::new(vec![Arc::new(claude), Arc::new(pi)]),
            git,
            zmx: ZmxService::new(Arc::new(ZmxCli::new(env.clone())), orb_root.join("zmx")),
        }
    };
    let git = services.git.clone();
    let harnesses = services.harnesses.clone();
    let zmx = services.zmx.clone();
    let sessions = spawn_sessions_actor(SessionsActorDeps {
        services,
        state: state.clone(),
        store,
        worktrees_root: worktrees_root.clone(),
        orb_root,
        incognito_root: PathBuf::from("/tmp/orb-incognito"),
        wake: frontend.waker(),
        integration_missing: !integrations.any_installed(),
    });
    runtime.block_on(sessions.wait_for_startup());
    // Stop migrated `--bg` sessions before any pane resumes them.
    let _ = runtime.block_on(async {
        sessions
            .ask(StopMigrated(|count| {
                let _ = writeln!(
                    std::io::stderr(),
                    "orb: stopping {count} old claude --bg sessions…"
                );
            }))
            .await
    });
    let worktrees = spawn_worktrees_actor(WorktreesActorDeps {
        git: git.clone(),
        state: state.clone(),
        worktrees_root: worktrees_root.clone(),
        wake: frontend.waker(),
        sweep_every: SWEEP_EVERY,
    });
    let search = spawn_search_actor(SearchActorDeps {
        state: state.clone(),
        harnesses: harnesses.clone(),
        index_path: search_index,
        wake: frontend.waker(),
    });
    frontend
        .run(
            state,
            sessions,
            worktrees,
            worktrees_root,
            search,
            git,
            harnesses,
            env,
            zmx,
            notifier,
        )
        .change_context(OrbError)
}

/// Where orb's integrations live: under `orb_root`, `claude_dir`, and pi's
/// agent dir (`$PI_CODING_AGENT_DIR`, else under `home`).
fn integration_paths(home: &Path, orb_root: &Path, claude_dir: &Path) -> IntegrationPaths {
    IntegrationPaths {
        orb_root: orb_root.to_owned(),
        claude_dir: claude_dir.to_owned(),
        pi_dir: integration::pi_agent_dir(
            std::env::var_os("PI_CODING_AGENT_DIR").map(PathBuf::from),
            home,
        ),
    }
}

/// `orb integration install`: installs both integrations and prints what it
/// wrote.
fn install_integrations(
    args: &[OsString],
    paths: &IntegrationPaths,
    home: &Path,
) -> Result<(), Report<OrbError>> {
    if args.get(1).is_none_or(|arg| arg != "install") || args.len() > 2 {
        return Err(Report::new(OrbError).attach("usage: orb integration install"));
    }
    let lines = integration::install(paths, home).change_context(OrbError)?;
    let mut out = std::io::stdout().lock();
    for line in lines {
        writeln!(out, "{line}").change_context(OrbError)?;
    }
    Ok(())
}

/// Desktop notices, clicked back to orb's window (kitty on macOS, niri on
/// Linux).
fn desktop_notifier() -> NotifierService {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let kitty = {
        let kitten = std::env::var_os("KITTY_INSTALLATION_DIR")
            .filter(|_| cfg!(target_os = "macos"))
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
    let niri = std::env::var_os("NIRI_SOCKET")
        .and_then(|_| on_path("niri", &path))
        .map(|niri| NiriTarget {
            niri,
            ancestors: own_ancestry(),
        });
    NotifierService::desktop(&path, ClickTarget { kitty, niri })
}

/// orb's process and its parents, for finding the window it runs in; just
/// orb when `ps` can't say.
fn own_ancestry() -> Vec<u32> {
    let parents = {
        let mut command = std::process::Command::new("ps");
        command.args(["-A", "-o", "pid=,ppid="]);
        match run_within(command, Duration::from_secs(2)) {
            Ok(Finished::Exited(output)) => parse_parents(&String::from_utf8_lossy(&output.stdout)),
            _ => HashMap::new(),
        }
    };
    ancestry(std::process::id(), &parents)
}
