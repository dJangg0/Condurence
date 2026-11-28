//! Git integration: worktree lifecycle, repo-level mutation locks, structured diffs and
//! FSEvents/inotify-backed dirty tracking.

pub mod cmd;
pub mod diff_engine;
pub mod files;
pub mod locks;
pub mod remotes;
pub mod watcher;
pub mod worktree;

use std::path::{Path, PathBuf};

use crate::error::{AppError, AppResult};

/// Name of the directory (inside the main repo) where child worktrees are provisioned.
pub const WORKTREES_DIR: &str = ".worktrees";

/// Resolves any checkout (main or linked worktree) to the canonical main repository root.
pub async fn repo_root(path: &Path) -> AppResult<PathBuf> {
    let common = cmd::run(path, &["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .await
        .map_err(|_| AppError::msg(format!("{} is not inside a git repository", path.display())))?;
    let common = PathBuf::from(common.trim());
    let root = if common.file_name().map(|n| n == ".git").unwrap_or(false) {
        common.parent().map(Path::to_path_buf).unwrap_or(common)
    } else {
        common
    };
    Ok(root.canonicalize().unwrap_or(root))
}

/// Ensures `.worktrees/` never shows up as untracked noise in the main checkout.
/// Uses `info/exclude` so the user's committed `.gitignore` stays untouched.
pub fn ensure_worktrees_excluded(repo_root: &Path) -> AppResult<()> {
    let exclude = repo_root.join(".git").join("info").join("exclude");
    // For a `.git` directory this is the right place; `.git` files (linked checkouts)
    // are never the main root, so repo_root always points at a real `.git` dir here.
    let entry = format!("/{WORKTREES_DIR}/");
    let current = std::fs::read_to_string(&exclude).unwrap_or_default();
    if current.lines().any(|line| line.trim() == entry) {
        return Ok(());
    }
    if let Some(parent) = exclude.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut next = current;
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    next.push_str(&entry);
    next.push('\n');
    std::fs::write(exclude, next)?;
    Ok(())
}
