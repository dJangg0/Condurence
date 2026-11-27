//! `#[tauri::command]` handlers. Each one validates its input and delegates to the
//! runtime. Names and payloads follow HANDOFF §6.4.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tauri::State;

use crate::auth::{self, ProviderAuth};
use crate::env::resolver;
use crate::error::{AppError, AppResult};
use crate::git::{self, diff_engine::FileDiff, worktree::{self, MergeResult, WorktreeInfo}};
use crate::github::{self, GitHubAccount, RepoConnection, RepoSummary};
use crate::graph::{self, AgentProvider, OrchestrationGraph};
use crate::models::{self, ModelOption};
use crate::pty::SpawnSpec;
use crate::pulls;
use crate::usage::{self, ProviderUsage};
use crate::runtime::{now_ms, state_db::{RepoRecord, RunEvent, RunRecord, StateDb}, CoreRuntime, RepoMetadata};
use crate::settings::{self, RepoGitSettings};
use crate::workspaces;

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
pub fn list_archived_repositories(runtime: Runtime<'_>) -> AppResult<Vec<RepoRecord>> {
    runtime.archived_repositories()
}

/// Hides a repository from Recent, or restores it. Never touches files on disk.
#[tauri::command]
pub fn archive_repository(runtime: Runtime<'_>, path: String, archived: bool) -> AppResult<()> {
    runtime.db.set_repo_archived(&path, archived)
}

/// Removes a repository from the app's lists. Never touches files on disk.
#[tauri::command]
pub fn forget_repository(runtime: Runtime<'_>, path: String) -> AppResult<()> {
    runtime.db.forget_repo(&path)
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

/// What archiving a workspace would discard and keep, for its confirmation. Changes nothing.
#[tauri::command]
pub async fn archive_preview(
    runtime: Runtime<'_>,
    repo_path: String,
    worktree_path: String,
    branch: Option<String>,
) -> AppResult<workspaces::ArchivePreview> {
    let settings = repo_settings(&runtime.db, &repo_path)?;
    workspaces::archive_preview(gh_binary().as_deref(), Path::new(&worktree_path), branch.as_deref(), &settings.remote).await
}

/// Archives a workspace on the user's request: removes its folder and, when the repository's
/// settings say so and nothing is lost, its branch. Uncommitted changes need `discard_changes`.
#[tauri::command]
pub async fn archive_workspace(
    runtime: Runtime<'_>,
    repo_path: String,
    branch: Option<String>,
    folder_name: String,
    discard_changes: bool,
) -> AppResult<workspaces::ArchiveOutcome> {
    let settings = repo_settings(&runtime.db, &repo_path)?;
    workspaces::archive_workspace(
        &runtime.locks,
        gh_binary().as_deref(),
        Path::new(&repo_path),
        branch.as_deref(),
        &folder_name,
        &settings.remote,
        settings.delete_branch_on_archive,
        discard_changes,
    )
    .await
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

/// What a branch would put in a PR: its commits since the base. Uncommitted edits are not included.
#[tauri::command]
pub async fn get_branch_diff(worktree_path: String, base: String) -> AppResult<Vec<FileDiff>> {
    if !settings::valid_name(&base) {
        crate::error::bail!("'{base}' is not a valid base");
    }
    git::diff_engine::branch_diff(Path::new(&worktree_path), &base).await
}

#[tauri::command]
pub async fn get_git_diff(worktree_path: String) -> AppResult<Vec<FileDiff>> {
    git::diff_engine::worktree_diff(&PathBuf::from(worktree_path)).await
}

/// Every file of a workspace that git lists, for the All files tab.
#[tauri::command]
pub async fn list_worktree_files(worktree_path: String) -> AppResult<git::files::FileListing> {
    git::files::list_files(Path::new(&worktree_path)).await
}

/// The text of one listed file. Ignored files, paths outside the workspace and links that lead out
/// of it are refused.
#[tauri::command]
pub async fn read_worktree_file(worktree_path: String, path: String) -> AppResult<git::files::FileContent> {
    git::files::read_file(Path::new(&worktree_path), &path).await
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

// ---------------------------------------------------------------- github

/// The `gh` binary. `CONDURENCE_GH_BIN` pins an explicit build, as it does for provider CLIs.
fn gh_binary() -> Option<PathBuf> {
    resolver::resolve_provider("gh")
}

/// Working directory for `gh` calls that are not tied to a repository.
fn home_or_root() -> PathBuf {
    crate::runtime::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

#[tauri::command]
pub async fn github_account_status() -> AppResult<GitHubAccount> {
    github::account_status(gh_binary().as_deref(), &home_or_root()).await
}

#[tauri::command]
pub async fn github_repo_connection(repo_path: String) -> AppResult<RepoConnection> {
    let root = git::repo_root(Path::new(&repo_path)).await?;
    github::repo_connection(gh_binary().as_deref(), &root).await
}

#[tauri::command]
pub async fn github_list_repositories() -> AppResult<Vec<RepoSummary>> {
    github::list_repositories(gh_binary().as_deref(), &home_or_root()).await
}

#[tauri::command]
pub async fn github_connect_repository(repo_path: String, name_with_owner: String) -> AppResult<RepoConnection> {
    let root = git::repo_root(Path::new(&repo_path)).await?;
    github::connect_repository(gh_binary().as_deref(), &root, &name_with_owner).await
}

/// Clones a repository from the signed-in account into `~/Condurence/repos` and returns its path.
/// A copy already cloned into the folder the app used before its rename is opened as it is.
#[tauri::command]
pub async fn github_clone_repository(name_with_owner: String) -> AppResult<String> {
    let home = crate::runtime::home_dir().ok_or_else(|| AppError::msg("cannot locate the home directory"))?;
    if let Some(existing) = github::existing_clone(&github::legacy_clone_root(&home), &name_with_owner).await {
        return Ok(existing.to_string_lossy().into_owned());
    }
    let root = github::default_clone_root(&home);
    let path = github::clone_repository(gh_binary().as_deref(), &root, &name_with_owner).await?;
    Ok(path.to_string_lossy().into_owned())
}

/// Runs `gh auth login` in a PTY. The frontend shows it in the normal terminal pane and
/// re-checks the account when the process exits.
#[tauri::command]
pub fn github_start_sign_in(runtime: Runtime<'_>, session_id: String) -> AppResult<()> {
    let gh = gh_binary().ok_or_else(|| AppError::msg(github::GH_MISSING))?;
    let cwd = home_or_root();
    runtime.db.record_pty_session(&session_id, None, None, &cwd, "gh auth login", now_ms())?;
    runtime.pty.spawn(SpawnSpec {
        session_id,
        cwd,
        program: gh.to_string_lossy().into_owned(),
        args: github::sign_in_args(),
        env: vec![],
        cols: DEFAULT_COLS,
        rows: DEFAULT_ROWS,
    })?;
    Ok(())
}

// ---------------------------------------------------------------- pull requests

#[tauri::command]
pub async fn pull_request_status(
    worktree_path: String,
    branch: String,
    remote: String,
) -> AppResult<Option<pulls::PullRequestStatus>> {
    pulls::status(gh_binary().as_deref(), Path::new(&worktree_path), &branch, &remote).await
}

/// Every PR in the repository with the branch it came from. The sidebar reads this once per repository
/// to show each workspace's merge state.
#[tauri::command]
pub async fn list_pull_requests(runtime: Runtime<'_>, repo_path: String) -> AppResult<Vec<pulls::BranchPull>> {
    let remote = repo_settings(&runtime.db, &repo_path)?.remote;
    pulls::list(gh_binary().as_deref(), Path::new(&repo_path), &remote).await
}

#[tauri::command]
pub async fn create_pull_request(
    worktree_path: String,
    branch: String,
    base: String,
    remote: String,
) -> AppResult<pulls::PullRequestStatus> {
    pulls::create(gh_binary().as_deref(), Path::new(&worktree_path), &branch, &base, &remote).await
}

#[tauri::command]
pub async fn merge_pull_request(worktree_path: String, branch: String, remote: String) -> AppResult<pulls::PullRequestStatus> {
    pulls::merge(gh_binary().as_deref(), Path::new(&worktree_path), &branch, &remote).await
}

#[tauri::command]
pub async fn sync_with_base(
    worktree_path: String,
    branch: String,
    base: String,
    remote: String,
) -> AppResult<pulls::SyncOutcome> {
    pulls::sync_with_base(Path::new(&worktree_path), &branch, &base, &remote).await
}

/// Opens Claude in the workspace on a `review` or `resolve` prompt. The prompt is built in the
/// backend from a fixed template, so the frontend never supplies agent instructions.
#[tauri::command]
pub fn start_agent_task(
    runtime: Runtime<'_>,
    session_id: String,
    worktree_path: String,
    task: String,
    base: String,
) -> AppResult<()> {
    let claude = resolver::resolve_provider("claude")
        .ok_or_else(|| AppError::msg("'claude' was not found on your PATH"))?;
    let cwd = PathBuf::from(&worktree_path);
    if !cwd.is_dir() {
        crate::error::bail!("'{worktree_path}' is not a directory");
    }
    let prompt = pulls::task_prompt(&task, &base)?;
    runtime.db.record_pty_session(&session_id, None, None, &cwd, "claude", now_ms())?;
    runtime.pty.spawn(SpawnSpec {
        session_id,
        cwd,
        program: claude.to_string_lossy().into_owned(),
        args: vec![prompt],
        env: vec![],
        cols: DEFAULT_COLS,
        rows: DEFAULT_ROWS,
    })?;
    Ok(())
}

/// Starts a review in a worktree with the agent the user chose and the context they configured. The
/// agent runs in a terminal that stays open, so the reviewer's findings are read where they appear.
#[tauri::command]
pub fn start_review(
    runtime: Runtime<'_>,
    session_id: String,
    worktree_path: String,
    base: String,
    agent: String,
    context: pulls::ReviewContext,
) -> AppResult<()> {
    let provider = parse_provider(&agent)?;
    if !matches!(provider, AgentProvider::Claude | AgentProvider::Codex) {
        crate::error::bail!("Reviews run with Claude or Codex");
    }
    let program = resolver::resolve_provider(provider.binary())
        .ok_or_else(|| AppError::msg(format!("'{}' was not found on your PATH", provider.binary())))?;
    let cwd = PathBuf::from(&worktree_path);
    if !cwd.is_dir() {
        crate::error::bail!("'{worktree_path}' is not a directory");
    }
    let prompt = pulls::review_prompt(&base, &context)?;
    runtime.db.record_pty_session(&session_id, None, None, &cwd, provider.binary(), now_ms())?;
    runtime.pty.spawn(SpawnSpec {
        session_id,
        cwd,
        program: program.to_string_lossy().into_owned(),
        args: vec![prompt],
        env: vec![],
        cols: DEFAULT_COLS,
        rows: DEFAULT_ROWS,
    })?;
    Ok(())
}

// ---------------------------------------------------------------- provider sign-in

/// Sign-in state for the Claude, Codex and OpenCode CLIs.
#[tauri::command]
pub async fn provider_auth_status() -> AppResult<Vec<ProviderAuth>> {
    let [a, b, c] = auth::SIGN_IN_PROVIDERS;
    let binaries = [
        resolver::resolve_provider(a.binary()),
        resolver::resolve_provider(b.binary()),
        resolver::resolve_provider(c.binary()),
    ];
    Ok(auth::all_statuses(binaries).await)
}

/// Runs the provider CLI's own login flow in a PTY, shown in the normal terminal pane.
#[tauri::command]
pub fn provider_start_sign_in(runtime: Runtime<'_>, session_id: String, provider: String) -> AppResult<()> {
    let parsed = parse_provider(&provider)?;
    let args = auth::sign_in_args(parsed)
        .ok_or_else(|| AppError::msg(format!("signing in to '{provider}' is not supported here")))?;
    let binary = resolver::resolve_provider(parsed.binary())
        .ok_or_else(|| AppError::msg(format!("'{}' was not found on your PATH", parsed.binary())))?;
    let cwd = home_or_root();
    let command = format!("{} {}", parsed.binary(), args.join(" "));
    runtime.db.record_pty_session(&session_id, None, None, &cwd, &command, now_ms())?;
    runtime.pty.spawn(SpawnSpec {
        session_id,
        cwd,
        program: binary.to_string_lossy().into_owned(),
        args,
        env: vec![],
        cols: DEFAULT_COLS,
        rows: DEFAULT_ROWS,
    })?;
    Ok(())
}

/// Models to offer in the topology inspector for a provider.
#[tauri::command]
pub async fn list_provider_models(provider: String) -> AppResult<Vec<ModelOption>> {
    let parsed = parse_provider(&provider)?;
    let binary = resolver::resolve_provider(parsed.binary());
    Ok(models::list_models(parsed, binary.as_deref(), &home_or_root()).await)
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

/// The Git settings of a repository, or the defaults until they are first saved.
fn repo_settings(db: &StateDb, repo_path: &str) -> AppResult<RepoGitSettings> {
    Ok(settings::parse(db.repo_settings_json(repo_path)?.as_deref()))
}

/// Creates a workspace the Conductor way: a champion-named branch from the repository's base.
#[tauri::command]
pub async fn create_workspace(runtime: Runtime<'_>, repo_path: String) -> AppResult<WorktreeInfo> {
    let settings = repo_settings(&runtime.db, &repo_path)?;
    let seed = uuid::Uuid::new_v4().as_u128() as u64;
    workspaces::create_workspace(&runtime.locks, Path::new(&repo_path), &settings, seed).await
}

#[tauri::command]
pub fn get_repo_git_settings(runtime: Runtime<'_>, repo_path: String) -> AppResult<RepoGitSettings> {
    repo_settings(&runtime.db, &repo_path)
}

#[tauri::command]
pub fn save_repo_git_settings(runtime: Runtime<'_>, repo_path: String, settings: RepoGitSettings) -> AppResult<()> {
    settings.validate()?;
    let json = serde_json::to_string(&settings)?;
    runtime.db.put_repo_settings_json(&repo_path, &json)
}

#[tauri::command]
pub async fn list_remotes(repo_path: String) -> AppResult<Vec<String>> {
    git::remotes::list_remotes(Path::new(&repo_path)).await
}

#[tauri::command]
pub async fn list_remote_branches(repo_path: String, remote: String) -> AppResult<Vec<String>> {
    if !settings::valid_name(&remote) {
        crate::error::bail!("'{remote}' is not a valid remote name");
    }
    git::remotes::list_remote_branches(Path::new(&repo_path), &remote).await
}

/// Runs started from a repository, newest first. Backs the run history.
#[tauri::command]
pub fn list_runs(runtime: Runtime<'_>, repo_path: String, limit: Option<i64>) -> AppResult<Vec<RunRecord>> {
    runtime.db.list_runs(&repo_path, limit.unwrap_or(20))
}

/// The worktrees a run's agents worked in, with their branches. The repository root is included
/// and flagged `isMain`, so the caller can leave it out of PR actions.
#[tauri::command]
pub async fn run_worktrees(runtime: Runtime<'_>, run_id: String) -> AppResult<Vec<WorktreeInfo>> {
    let Some(inputs) = runtime.db.run_usage_inputs(&run_id)? else {
        return Ok(Vec::new());
    };
    let all = worktree::list_worktrees(Path::new(&inputs.repo_path)).await?;
    Ok(select_run_worktrees(all, &inputs.cwds))
}

/// Keeps the worktrees whose path is one of the run's working directories, in git's order.
pub fn select_run_worktrees(all: Vec<WorktreeInfo>, cwds: &[String]) -> Vec<WorktreeInfo> {
    let wanted: std::collections::BTreeSet<PathBuf> = cwds.iter().map(|cwd| canonical_path(cwd)).collect();
    all.into_iter().filter(|info| wanted.contains(&canonical_path(&info.path))).collect()
}

fn canonical_path(path: &str) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path))
}

/// Moves a workspace that sits on the base branch onto a new branch, so its work can be a PR.
/// Its commits and uncommitted edits come along. On the user's explicit request only.
#[tauri::command]
pub async fn start_branch(worktree_path: String, branch: String) -> AppResult<()> {
    pulls::start_branch(Path::new(&worktree_path), &branch).await
}

/// Commits a worktree's changes with `message`, on the user's explicit request. Returns the new
/// commit, or `None` when there was nothing to commit.
#[tauri::command]
pub async fn commit_worktree_changes(worktree_path: String, message: String) -> AppResult<Option<String>> {
    pulls::commit_changes(Path::new(&worktree_path), &message).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn worktree(path: &str, branch: Option<&str>, is_main: bool) -> WorktreeInfo {
        WorktreeInfo {
            path: path.into(),
            branch: branch.map(str::to_string),
            head: "abc".into(),
            is_main,
            locked: false,
        }
    }

    #[test]
    fn a_run_sees_only_its_own_worktrees_including_the_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        let child = dir.path().join("repo/.worktrees/a-greeter-1");
        let other = dir.path().join("other");
        for path in [&root, &child, &other] {
            std::fs::create_dir_all(path).unwrap();
        }
        let all = vec![
            worktree(root.to_str().unwrap(), Some("main"), true),
            worktree(child.to_str().unwrap(), Some("oc/a/greeter-1"), false),
            worktree(other.to_str().unwrap(), Some("feature"), false),
        ];
        // The run recorded the root (as a session's cwd) and the child, but not the other worktree.
        let cwds = vec![root.to_string_lossy().into_owned(), child.to_string_lossy().into_owned()];
        let picked = select_run_worktrees(all, &cwds);
        assert_eq!(picked.iter().map(|w| w.branch.as_deref()).collect::<Vec<_>>(), [Some("main"), Some("oc/a/greeter-1")]);
        assert!(picked[0].is_main);
    }
}

/// Token usage each provider's CLI recorded for one run. Unknown runs report no rows.
#[tauri::command]
pub async fn run_usage(runtime: Runtime<'_>, run_id: String) -> AppResult<Vec<ProviderUsage>> {
    let Some(inputs) = runtime.db.run_usage_inputs(&run_id)? else {
        return Ok(Vec::new());
    };
    let home = crate::runtime::home_dir().ok_or_else(|| AppError::msg("cannot locate the home directory"))?;
    let reader = runtime.usage.clone();
    let now = now_ms();
    // Reading session logs touches many files, so keep it off the async runtime's threads.
    tokio::task::spawn_blocking(move || usage::measure_run(&reader, &home, &inputs, now))
        .await
        .map_err(|error| AppError::msg(format!("usage scan failed: {error}")))
}

/// Token usage per provider across every session on this Mac that started in the last
/// `usage::SUMMARY_DAYS` days. Backs the token usage section of the profile pane.
#[tauri::command]
pub async fn provider_usage_summary(runtime: Runtime<'_>) -> AppResult<Vec<ProviderUsage>> {
    let home = crate::runtime::home_dir().ok_or_else(|| AppError::msg("cannot locate the home directory"))?;
    let reader = runtime.usage.clone();
    let now = now_ms();
    tokio::task::spawn_blocking(move || usage::summarize(&reader, &home, usage::SUMMARY_DAYS, now))
        .await
        .map_err(|error| AppError::msg(format!("usage scan failed: {error}")))
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
