//! Condurence core: repo/worktree management, PTY sessions, structured diffs and the
//! MCP router that turns a canvas topology into live multi-agent execution.

pub mod auth;
pub mod env;
pub mod error;
pub mod gates;
pub mod git;
pub mod github;
pub mod graph;
pub mod ipc;
pub mod mcp;
pub mod models;
pub mod pty;
pub mod pulls;
pub mod runtime;
pub mod settings;
pub mod usage;
pub mod workspaces;

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
            ipc::commands::list_archived_repositories,
            ipc::commands::archive_repository,
            ipc::commands::forget_repository,
            ipc::commands::load_graph,
            ipc::commands::save_graph,
            ipc::commands::list_worktrees,
            ipc::commands::create_worktree,
            ipc::commands::archive_preview,
            ipc::commands::archive_workspace,
            ipc::commands::merge_worktree,
            ipc::commands::get_git_diff,
            ipc::commands::github_account_status,
            ipc::commands::github_repo_connection,
            ipc::commands::github_list_repositories,
            ipc::commands::github_connect_repository,
            ipc::commands::github_clone_repository,
            ipc::commands::github_start_sign_in,
            ipc::commands::pull_request_status,
            ipc::commands::list_pull_requests,
            ipc::commands::create_pull_request,
            ipc::commands::merge_pull_request,
            ipc::commands::sync_with_base,
            ipc::commands::start_agent_task,
            ipc::commands::provider_auth_status,
            ipc::commands::provider_start_sign_in,
            ipc::commands::list_provider_models,
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
            ipc::commands::list_runs,
            ipc::commands::run_worktrees,
            ipc::commands::commit_worktree_changes,
            ipc::commands::start_branch,
            ipc::commands::create_workspace,
            ipc::commands::get_branch_diff,
            ipc::commands::list_worktree_files,
            ipc::commands::read_worktree_file,
            ipc::commands::start_review,
            ipc::commands::get_repo_git_settings,
            ipc::commands::save_repo_git_settings,
            ipc::commands::list_remotes,
            ipc::commands::list_remote_branches,
            ipc::commands::run_usage,
            ipc::commands::provider_usage_summary,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Condurence");

    app.run(|app_handle, event| {
        if let RunEvent::Exit = event {
            if let Some(runtime) = app_handle.try_state::<Arc<CoreRuntime>>() {
                runtime.shutdown();
            }
        }
    });
}
