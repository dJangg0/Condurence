//! Debounced per-worktree file watcher (FSEvents on macOS, inotify on Linux,
//! ReadDirectoryChangesW on Windows). Emits `workspace-dirty://{workspaceId}` with the
//! current changed-file count after bursts of edits settle.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use notify_debouncer_mini::notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{new_debouncer, DebounceEventResult, Debouncer};
use serde_json::json;

use super::diff_engine::changed_file_count;
use crate::error::AppResult;
use crate::runtime::events::EventSink;

const DEBOUNCE: Duration = Duration::from_millis(150);
/// Directories whose churn must never wake the dirty tracker.
const IGNORED_COMPONENTS: &[&str] = &[".git", "node_modules", "target", ".next", ".worktrees"];

#[derive(Default)]
pub struct WorkspaceWatchers {
    watchers: DashMap<PathBuf, Debouncer<RecommendedWatcher>>,
}

impl WorkspaceWatchers {
    /// Starts watching `worktree` (idempotent) and returns its stable workspace id.
    pub fn watch(&self, events: Arc<dyn EventSink>, worktree: PathBuf) -> AppResult<String> {
        let id = workspace_id(&worktree);
        if self.watchers.contains_key(&worktree) {
            return Ok(id);
        }

        let root = worktree.clone();
        let event_name = format!("workspace-dirty://{id}");
        let mut debouncer = new_debouncer(DEBOUNCE, move |result: DebounceEventResult| {
            let Ok(changes) = result else { return };
            let relevant = changes.iter().any(|change| !is_ignored(&root, &change.path));
            if !relevant {
                return;
            }
            let count = changed_file_count(&root).unwrap_or(0);
            events.emit(&event_name, json!({ "changedFilesCount": count }));
        })
        .map_err(|e| crate::error::AppError::msg(format!("watcher init failed: {e}")))?;

        debouncer
            .watcher()
            .watch(&worktree, RecursiveMode::Recursive)
            .map_err(|e| crate::error::AppError::msg(format!("cannot watch {}: {e}", worktree.display())))?;
        self.watchers.insert(worktree, debouncer);
        Ok(id)
    }

    pub fn unwatch(&self, worktree: &Path) {
        self.watchers.remove(worktree);
    }
}

/// Deterministic, URL-safe id derived from the worktree path.
pub fn workspace_id(worktree: &Path) -> String {
    let mut hasher = DefaultHasher::new();
    worktree.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn is_ignored(root: &Path, path: &Path) -> bool {
    let relative = path.strip_prefix(root).unwrap_or(path);
    relative.components().any(|component| {
        component
            .as_os_str()
            .to_str()
            .map(|name| IGNORED_COMPONENTS.contains(&name))
            .unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_build_and_git_directories() {
        let root = Path::new("/repo/.worktrees/ws");
        assert!(is_ignored(root, Path::new("/repo/.worktrees/ws/node_modules/x.js")));
        assert!(is_ignored(root, Path::new("/repo/.worktrees/ws/.git/index")));
        assert!(!is_ignored(root, Path::new("/repo/.worktrees/ws/src/main.rs")));
    }

    #[test]
    fn workspace_id_is_stable_and_hex() {
        let a = workspace_id(Path::new("/tmp/a"));
        assert_eq!(a, workspace_id(Path::new("/tmp/a")));
        assert_eq!(a.len(), 16);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
