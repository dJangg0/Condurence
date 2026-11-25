//! Repo-level mutation semaphores.
//!
//! Mutating git commands (`worktree add/remove`, `branch`, `merge`, `fetch`) all contend
//! on `.git/index.lock` and the shared refs database when run against the same repository.
//! Each main repository root gets a single-permit semaphore. Read-only commands such as
//! `git status` and `git diff` run concurrently and never take a permit.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use dashmap::DashMap;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

#[derive(Default)]
pub struct RepoLocks {
    semaphores: DashMap<PathBuf, Arc<Semaphore>>,
}

impl RepoLocks {
    /// Waits for exclusive mutation rights on `repo_root`. Hold the permit for the
    /// duration of the mutating command(s).
    pub async fn acquire(&self, repo_root: &Path) -> OwnedSemaphorePermit {
        let semaphore = self
            .semaphores
            .entry(repo_root.to_path_buf())
            .or_insert_with(|| Arc::new(Semaphore::new(1)))
            .clone();
        semaphore
            .acquire_owned()
            .await
            .expect("repo semaphores are never closed")
    }
}
