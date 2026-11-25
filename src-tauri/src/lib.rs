//! OpenConductor core: repo/worktree management, PTY sessions, structured diffs and the
//! MCP router that turns a canvas topology into live multi-agent execution.

pub mod env;
pub mod error;
pub mod git;
pub mod graph;
pub mod ipc;
pub mod mcp;
pub mod pty;
pub mod runtime;

use std::sync::Arc;

use tauri::{Manager, RunEvent};

use runtime::CoreRuntime;

pub fn run() {
    // The same binary doubles as the stdio MCP bridge that provider CLIs spawn. This must
    // run before any GUI initialisation.
    if let Some(code) = mcp::bridge::run_if_requested(std::env::args().skip(1)) {
        std::process::exit(code);
    }

    // GUI apps launched from Finder inherit a minimal PATH. Hydrate it once, before any
    // thread exists, so git, PTYs and provider CLIs see the user's real tools.
    env::resolver::hydrate_process_env();

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let runtime = CoreRuntime::new(app.handle().clone())?;
            app.manage(Arc::new(runtime));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            ipc::commands::open_repository,
            ipc::commands::list_recent_repositories,
            ipc::commands::load_graph,
            ipc::commands::save_graph,
            ipc::commands::list_worktrees,
            ipc::commands::create_worktree,
            ipc::commands::remove_worktree,
            ipc::commands::merge_worktree,
            ipc::commands::get_git_diff,
            ipc::commands::watch_workspace,
            ipc::commands::unwatch_workspace,
            ipc::commands::spawn_agent_pty,
            ipc::commands::write_agent_pty,
            ipc::commands::resize_agent_pty,
            ipc::commands::kill_agent_pty,
            ipc::commands::get_pty_history,
            ipc::commands::start_orchestration_run,
            ipc::commands::stop_orchestration_run,
            ipc::commands::resolve_approval_gate,
            ipc::commands::list_run_events,
        ])
        .build(tauri::generate_context!())
        .expect("error while building OpenConductor");

    app.run(|app_handle, event| {
        if let RunEvent::Exit = event {
            if let Some(runtime) = app_handle.try_state::<Arc<CoreRuntime>>() {
                runtime.shutdown();
            }
        }
    });
}
