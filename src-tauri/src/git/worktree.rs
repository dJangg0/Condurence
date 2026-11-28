//! Worktree lifecycle: create, list, remove and merge.
//!
//! A workspace owns a physical `git worktree` at `<repo>/.worktrees/<folder>` on branch
//! `<branch>`. Creation, removal and merges take the repo-level lock; listing does not.

use std::path::{Path, PathBuf};

use serde::Serialize;

use super::{cmd, ensure_worktrees_excluded, locks::RepoLocks, repo_root, WORKTREES_DIR};
use crate::error::{AppError, AppResult};

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeInfo {
    pub path: String,
    pub branch: Option<String>,
    pub head: String,
    pub is_main: bool,
    pub locked: bool,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MergeResult {
    pub merged: bool,
    pub commit: Option<String>,
    pub conflicts: Vec<String>,
    pub message: String,
}

/// Parses `git worktree list --porcelain`. The first record is always the main checkout.
pub fn parse_worktree_list(text: &str) -> Vec<WorktreeInfo> {
    let mut out = Vec::new();
    let mut current: Option<WorktreeInfo> = None;
    for line in text.lines().chain(std::iter::once("")) {
        if line.is_empty() {
            if let Some(info) = current.take() {
                out.push(info);
            }
            continue;
        }
        if let Some(path) = line.strip_prefix("worktree ") {
            if let Some(info) = current.take() {
                out.push(info);
            }
            current = Some(WorktreeInfo {
                path: path.to_string(),
                branch: None,
                head: String::new(),
                is_main: out.is_empty(),
                locked: false,
            });
        } else if let Some(info) = current.as_mut() {
            if let Some(head) = line.strip_prefix("HEAD ") {
                info.head = head.to_string();
            } else if let Some(branch) = line.strip_prefix("branch refs/heads/") {
                info.branch = Some(branch.to_string());
            } else if line.starts_with("locked") {
                info.locked = true;
            }
        }
    }
    out
}

pub async fn list_worktrees(repo_path: &Path) -> AppResult<Vec<WorktreeInfo>> {
    let root = repo_root(repo_path).await?;
    let text = cmd::run(&root, &["worktree", "list", "--porcelain"]).await?;
    Ok(parse_worktree_list(&text))
}

/// Branch names are restricted to a conservative subset of git's ref rules.
pub fn is_valid_branch(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 200
        && !name.starts_with(['-', '/', '.'])
        && !name.ends_with(['/', '.'])
        && !name.contains("..")
        && !name.contains("//")
        && !name.ends_with(".lock")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'))
}

pub fn is_valid_folder(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 120
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// Creates a workspace worktree. Reuses an existing branch when one already exists.
/// `start_point` defaults to HEAD and is only honoured when creating a new branch.
pub async fn create_worktree(
    locks: &RepoLocks,
    repo_path: &Path,
    branch_name: &str,
    folder_name: &str,
    start_point: Option<&str>,
) -> AppResult<WorktreeInfo> {
    if !is_valid_branch(branch_name) {
        crate::error::bail!("'{branch_name}' is not a valid branch name");
    }
    if !is_valid_folder(folder_name) {
        crate::error::bail!("'{folder_name}' is not a valid workspace folder name");
    }

    let root = repo_root(repo_path).await?;
    ensure_worktrees_excluded(&root)?;
    let target: PathBuf = root.join(WORKTREES_DIR).join(folder_name);
    if target.exists() {
        crate::error::bail!("Workspace folder '{folder_name}' already exists");
    }
    std::fs::create_dir_all(root.join(WORKTREES_DIR))?;
    let target_str = target.to_string_lossy().into_owned();

    let _permit = locks.acquire(&root).await;
    let branch_ref = format!("refs/heads/{branch_name}");
    let branch_exists = cmd::raw(&root, &["show-ref", "--verify", "--quiet", &branch_ref])
        .await?
        .status
        .success();

    let mut args: Vec<&str> = vec![
        "-c",
        "core.longpaths=true",
        "-c",
        "core.autocrlf=input",
        "worktree",
        "add",
    ];
    if branch_exists {
        args.extend([target_str.as_str(), branch_name]);
    } else {
        args.extend(["-b", branch_name, target_str.as_str(), start_point.unwrap_or("HEAD")]);
    }
    cmd::run(&root, &args).await?;

    let created = canonical(&target);
    list_worktrees(&root)
        .await?
        .into_iter()
        .find(|wt| canonical(Path::new(&wt.path)) == created)
        .ok_or_else(|| AppError::msg("worktree was created but could not be listed"))
}

/// Merges `branch` into `target` in the main checkout. The main checkout must already be
/// on `target`. Conflicts are reported and the merge is aborted, leaving the tree clean.
pub async fn merge_worktree(
    locks: &RepoLocks,
    repo_path: &Path,
    branch: &str,
    target: &str,
    squash: bool,
) -> AppResult<MergeResult> {
    if !is_valid_branch(branch) || !is_valid_branch(target) {
        crate::error::bail!("Invalid branch name");
    }
    let root = repo_root(repo_path).await?;
    let current = cmd::run(&root, &["rev-parse", "--abbrev-ref", "HEAD"]).await?;
    if current.trim() != target {
        crate::error::bail!("Check out '{target}' in {} before merging", root.display());
    }

    let _permit = locks.acquire(&root).await;
    if squash {
        if let Err(err) = cmd::run(&root, &["merge", "--squash", branch]).await {
            let conflicts = conflicted_files(&root).await;
            let _ = cmd::raw(&root, &["reset", "--merge"]).await;
            return Ok(MergeResult {
                merged: false,
                commit: None,
                conflicts,
                message: err.to_string(),
            });
        }
        let staged = cmd::raw(&root, &["diff", "--cached", "--quiet"]).await?;
        if staged.status.success() {
            return Ok(MergeResult {
                merged: false,
                commit: None,
                conflicts: vec![],
                message: "Nothing to merge: the branch has no new changes".into(),
            });
        }
        let message = format!("Squash merge {branch}");
        cmd::run(&root, &["commit", "-m", message.as_str()]).await?;
    } else {
        let message = format!("Merge branch '{branch}'");
        if let Err(err) = cmd::run(&root, &["merge", "--no-ff", "--no-edit", "-m", message.as_str(), branch]).await {
            let conflicts = conflicted_files(&root).await;
            let _ = cmd::raw(&root, &["merge", "--abort"]).await;
            return Ok(MergeResult {
                merged: false,
                commit: None,
                conflicts,
                message: err.to_string(),
            });
        }
    }

    let head = cmd::run(&root, &["rev-parse", "HEAD"]).await?.trim().to_string();
    Ok(MergeResult {
        merged: true,
        commit: Some(head.clone()),
        conflicts: vec![],
        message: format!("Merged {branch} into {target} at {}", &head[..head.len().min(8)]),
    })
}

async fn conflicted_files(root: &Path) -> Vec<String> {
    cmd::run(root, &["diff", "--name-only", "--diff-filter=U"])
        .await
        .map(|text| text.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_porcelain_worktree_list() {
        let text = "worktree /repo\nHEAD aaa\nbranch refs/heads/main\n\nworktree /repo/.worktrees/ws1\nHEAD bbb\nbranch refs/heads/feature/x\nlocked\n\nworktree /repo/.worktrees/ws2\nHEAD ccc\ndetached\n";
        let list = parse_worktree_list(text);
        assert_eq!(list.len(), 3);
        assert!(list[0].is_main);
        assert_eq!(list[0].branch.as_deref(), Some("main"));
        assert_eq!(list[1].branch.as_deref(), Some("feature/x"));
        assert!(list[1].locked);
        assert_eq!(list[2].branch, None);
        assert!(!list[2].is_main);
    }

    #[test]
    fn validates_branch_and_folder_names() {
        assert!(is_valid_branch("oc/abc123/senior-dev-1"));
        assert!(!is_valid_branch("../escape"));
        assert!(!is_valid_branch("bad..name"));
        assert!(!is_valid_branch("-flag"));
        assert!(is_valid_folder("ws-1_a.b"));
        assert!(!is_valid_folder("../x"));
        assert!(!is_valid_folder(".hidden"));
    }
}
