//! `#[tauri::command]` handlers. Each one validates its input and delegates to the
//! runtime. Names and payloads follow HANDOFF §6.4.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tauri::State;

use crate::env::resolver;
use crate::error::{AppError, AppResult};
use crate::git::{self, diff_engine::FileDiff, worktree::{self, MergeResult, WorktreeInfo}};
use crate::graph::{self, AgentProvider, OrchestrationGraph};
use crate::pty::SpawnSpec;
use crate::runtime::{now_ms, state_db::{RepoRecord, RunEvent}, CoreRuntime, RepoMetadata};

type Runtime<'a> = State<'a, Arc<CoreRuntime>>;

const DEFAULT_COLS: u16 = 120;
const DEFAULT_ROWS: u16 = 32;

// ---------------------------------------------------------------- repositories

#[tauri::command]
pub async fn open_repository(runtime: Runtime<'_>, path: String) -> AppResult<RepoMetadata> {
    runtime.open_repository(&path).await
}

#[tauri::command]
pub fn list_recent_repositories(runtime: Runtime<'_>) -> AppResult<Vec<RepoRecord>> {
    runtime.recent_repositories()
}

#[tauri::command]
pub async fn load_graph(repo_path: String) -> AppResult<OrchestrationGraph> {
    let root = git::repo_root(Path::new(&repo_path)).await?;
    graph::load(&root)
}

#[tauri::command]
pub async fn save_graph(repo_path: String, graph: OrchestrationGraph) -> AppResult<()> {
    let root = git::repo_root(Path::new(&repo_path)).await?;
    graph::save(&root, &graph)
}

// ---------------------------------------------------------------- worktrees

#[tauri::command]
pub async fn list_worktrees(repo_path: String) -> AppResult<Vec<WorktreeInfo>> {
    worktree::list_worktrees(Path::new(&repo_path)).await
}

#[tauri::command]
pub async fn create_worktree(
    runtime: Runtime<'_>,
    repo_path: String,
    branch_name: String,
    folder_name: String,
    start_point: Option<String>,
) -> AppResult<WorktreeInfo> {
    worktree::create_worktree(
        &runtime.locks,
        Path::new(&repo_path),
        &branch_name,
        &folder_name,
        start_point.as_deref(),
    )
    .await
}

#[tauri::command]
pub async fn remove_worktree(
    runtime: Runtime<'_>,
    repo_path: String,
    branch_name: String,
    folder_name: String,
) -> AppResult<()> {
    worktree::remove_worktree(&runtime.locks, Path::new(&repo_path), &branch_name, &folder_name).await
}

#[tauri::command]
pub async fn merge_worktree(
    runtime: Runtime<'_>,
    repo_path: String,
    branch_name: String,
    target_branch: String,
    squash: bool,
) -> AppResult<MergeResult> {
    worktree::merge_worktree(&runtime.locks, Path::new(&repo_path), &branch_name, &target_branch, squash).await
}

#[tauri::command]
pub async fn get_git_diff(worktree_path: String) -> AppResult<Vec<FileDiff>> {
    git::diff_engine::worktree_diff(&PathBuf::from(worktree_path)).await
}

/// Starts the debounced watcher for a worktree and returns its workspace id.
#[tauri::command]
pub fn watch_workspace(runtime: Runtime<'_>, worktree_path: String) -> AppResult<String> {
    runtime
        .watchers
        .watch(runtime.events.clone(), PathBuf::from(worktree_path))
}

#[tauri::command]
pub fn unwatch_workspace(runtime: Runtime<'_>, worktree_path: String) {
    runtime.watchers.unwatch(Path::new(&worktree_path));
}

// ---------------------------------------------------------------- terminals

/// Spawns an interactive terminal. `provider` is `"shell"` for the user's login shell or
/// an agent provider name (`claude`, `codex`, `opencode`, `gemini`) for its TUI.
#[tauri::command]
pub fn spawn_agent_pty(
    runtime: Runtime<'_>,
    session_id: String,
    cwd: String,
    provider: String,
    cols: Option<u16>,
    rows: Option<u16>,
) -> AppResult<()> {
    let cwd_path = PathBuf::from(&cwd);
    if !cwd_path.is_dir() {
        crate::error::bail!("'{cwd}' is not a directory");
    }
    let (program, args) = if provider == "shell" {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        (shell, vec!["-l".to_string()])
    } else {
        let parsed = parse_provider(&provider)?;
        let binary = resolver::resolve_provider(parsed.binary()).ok_or_else(|| {
            AppError::msg(format!("'{}' was not found on your PATH", parsed.binary()))
        })?;
        (binary.to_string_lossy().into_owned(), vec![])
    };

    runtime.db.record_pty_session(&session_id, None, None, &cwd_path, &program, now_ms())?;
    runtime.pty.spawn(SpawnSpec {
        session_id,
        cwd: cwd_path,
        program,
        args,
        env: vec![],
        cols: cols.unwrap_or(DEFAULT_COLS),
        rows: rows.unwrap_or(DEFAULT_ROWS),
    })?;
    Ok(())
}

#[tauri::command]
pub fn write_agent_pty(runtime: Runtime<'_>, session_id: String, data: String) -> AppResult<()> {
    runtime.pty.get(&session_id)?.write(data.as_bytes())
}

#[tauri::command]
pub fn resize_agent_pty(runtime: Runtime<'_>, session_id: String, cols: u16, rows: u16) -> AppResult<()> {
    runtime.pty.get(&session_id)?.resize(cols, rows)
}

#[tauri::command]
pub async fn kill_agent_pty(runtime: Runtime<'_>, session_id: String) -> AppResult<()> {
    let pty = runtime.pty.clone();
    tokio::task::spawn_blocking(move || pty.terminate(&session_id))
        .await
        .map_err(|e| AppError::msg(e.to_string()))
}

#[tauri::command]
pub fn get_pty_history(runtime: Runtime<'_>, session_id: String) -> AppResult<String> {
    Ok(runtime.pty.get(&session_id)?.history())
}

// ---------------------------------------------------------------- orchestration

#[tauri::command]
pub async fn start_orchestration_run(
    runtime: Runtime<'_>,
    repo_path: String,
    root_node_id: String,
    user_goal: String,
) -> AppResult<String> {
    let root = git::repo_root(Path::new(&repo_path)).await?;
    let topology = graph::load(&root)?;
    runtime
        .router
        .start_run(root, topology, root_node_id, user_goal)
        .await
}

#[tauri::command]
pub async fn stop_orchestration_run(runtime: Runtime<'_>, run_id: String) -> AppResult<()> {
    runtime.router.stop_run(&run_id).await
}

#[tauri::command]
pub fn resolve_approval_gate(runtime: Runtime<'_>, run_id: String, edge_id: String, approved: bool) -> AppResult<()> {
    runtime.router.resolve_approval(&run_id, &edge_id, approved)
}

#[tauri::command]
pub fn list_run_events(runtime: Runtime<'_>, run_id: String, limit: Option<i64>) -> AppResult<Vec<RunEvent>> {
    runtime.db.list_events(&run_id, limit.unwrap_or(500))
}

fn parse_provider(name: &str) -> AppResult<AgentProvider> {
    match name {
        "claude" => Ok(AgentProvider::Claude),
        "codex" => Ok(AgentProvider::Codex),
        "opencode" => Ok(AgentProvider::Opencode),
        "gemini" => Ok(AgentProvider::Gemini),
        other => crate::error::bail!("unknown provider '{other}'"),
    }
}
