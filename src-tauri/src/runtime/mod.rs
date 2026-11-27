//! Core runtime: owns every long-lived subsystem and coordinates startup and shutdown.

pub mod events;
pub mod recovery;
pub mod state_db;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::AppHandle;

use crate::error::{AppError, AppResult};
use crate::git::{self, locks::RepoLocks, watcher::WorkspaceWatchers};
use crate::graph;
use crate::mcp::{self, router::Router};
use crate::pty::PtyManager;
use crate::usage::UsageReader;
use events::EventSink;
use state_db::{RepoRecord, StateDb};

pub struct CoreRuntime {
    pub events: Arc<dyn EventSink>,
    pub db: Arc<StateDb>,
    pub pty: Arc<PtyManager>,
    pub locks: Arc<RepoLocks>,
    pub watchers: Arc<WorkspaceWatchers>,
    pub router: Arc<Router>,
    /// Shared parse cache for provider session logs, used by the run meter and the profile pane.
    pub usage: Arc<UsageReader>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RepoMetadata {
    pub path: String,
    pub name: String,
    pub current_branch: String,
    pub has_graph: bool,
}

impl CoreRuntime {
    pub fn new(app: AppHandle) -> AppResult<Self> {
        let events: Arc<dyn EventSink> = Arc::new(app);
        let db = Arc::new(StateDb::open(&state_db_path()?)?);
        // Nothing can still be running at startup, so runs the last launch left live are over.
        let launched = now_ms();
        db.close_interrupted_runs(launched)?;
        // A crash skips the exit hook, and agents run in their own process groups, so the ones the
        // last launch started may still be running. Stop them before this launch starts any of its own.
        match recovery::recover(&db, launched, &mcp::compiler::run_dirs_root(), std::time::Duration::from_millis(1500)) {
            Ok(report) if !report.is_empty() => eprintln!("recovered from an unclean exit: {report:?}"),
            Ok(_) => {}
            Err(e) => eprintln!("could not clean up after the last launch: {e}"),
        }
        let pty = Arc::new(PtyManager::new(events.clone()));
        let locks = Arc::new(RepoLocks::default());
        let watchers = Arc::new(WorkspaceWatchers::default());

        // Bind first so the port is known before any run is started.
        let (listener, port) = mcp::server::bind_loopback()?;
        let bridge_exe = std::env::current_exe()?;
        let router = Arc::new(Router::new(
            events.clone(),
            db.clone(),
            pty.clone(),
            locks.clone(),
            port,
            bridge_exe,
        ));
        mcp::server::serve(listener, router.clone())?;

        Ok(Self {
            events,
            db,
            pty,
            locks,
            watchers,
            router,
            usage: Arc::new(UsageReader::default()),
        })
    }

    /// Normalizes any path inside a repository to its main root and records it as recent.
    pub async fn open_repository(&self, path: &str) -> AppResult<RepoMetadata> {
        let root = git::repo_root(Path::new(path)).await?;
        let branch = git::cmd::run(&root, &["rev-parse", "--abbrev-ref", "HEAD"])
            .await
            .unwrap_or_default()
            .trim()
            .to_string();
        let name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.display().to_string());
        let root_str = root.to_string_lossy().into_owned();
        self.db.upsert_repo(&root_str, &name, now_ms())?;

        Ok(RepoMetadata {
            path: root_str,
            name,
            current_branch: if branch.is_empty() { "HEAD".into() } else { branch },
            has_graph: root.join(graph::GRAPH_RELATIVE_PATH).is_file(),
        })
    }

    pub fn recent_repositories(&self) -> AppResult<Vec<RepoRecord>> {
        self.db.recent_repos(200)
    }

    pub fn archived_repositories(&self) -> AppResult<Vec<RepoRecord>> {
        self.db.archived_repos(100)
    }

    /// Graceful shutdown: cancel runs first so awaiting handoffs unwind, then terminate
    /// every PTY process group so no compiler, test runner or dev server is orphaned.
    pub fn shutdown(&self) {
        self.router.shutdown();
        self.pty.terminate_all();
    }
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// Condurence's folder in the home directory, `~/.condurence`: the state database and the global
/// skills. A folder left by the app's earlier name, `~/.openconductor`, is moved over the first
/// time, so the run history, the repository list and the skills carry on.
pub fn data_dir() -> Option<PathBuf> {
    home_dir().map(|home| migrate_data_dir(&home))
}

/// The data folder under `home`. When only the earlier folder exists it is renamed, which is atomic
/// on one volume and takes the database's WAL files with it. If it cannot be moved, that folder
/// keeps being used, so nothing is left behind and the app does not start with an empty history.
pub fn migrate_data_dir(home: &Path) -> PathBuf {
    let current = home.join(".condurence");
    let earlier = home.join(".openconductor");
    if !current.exists() && earlier.is_dir() && std::fs::rename(&earlier, &current).is_err() {
        return earlier;
    }
    current
}

fn state_db_path() -> AppResult<PathBuf> {
    let dir = data_dir().ok_or_else(|| AppError::msg("cannot locate the home directory"))?;
    Ok(dir.join("state.db"))
}

#[cfg(test)]
mod data_dir_tests {
    use super::*;

    #[test]
    fn the_earlier_folder_is_moved_over_with_everything_in_it() {
        let home = tempfile::tempdir().unwrap();
        let earlier = home.path().join(".openconductor");
        std::fs::create_dir_all(earlier.join("skills")).unwrap();
        std::fs::write(earlier.join("state.db"), "runs").unwrap();
        std::fs::write(earlier.join("state.db-wal"), "pending writes").unwrap();
        std::fs::write(earlier.join("skills").join("planner.md"), "plan").unwrap();

        let dir = migrate_data_dir(home.path());

        assert_eq!(dir, home.path().join(".condurence"));
        assert!(!earlier.exists(), "the earlier folder is gone, not copied");
        assert_eq!(std::fs::read_to_string(dir.join("state.db")).unwrap(), "runs");
        assert_eq!(std::fs::read_to_string(dir.join("state.db-wal")).unwrap(), "pending writes");
        assert_eq!(std::fs::read_to_string(dir.join("skills").join("planner.md")).unwrap(), "plan");
    }

    #[test]
    fn a_folder_that_already_exists_is_never_overwritten() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".condurence")).unwrap();
        std::fs::write(home.path().join(".condurence").join("state.db"), "new").unwrap();
        std::fs::create_dir_all(home.path().join(".openconductor")).unwrap();
        std::fs::write(home.path().join(".openconductor").join("state.db"), "old").unwrap();

        let dir = migrate_data_dir(home.path());

        assert_eq!(std::fs::read_to_string(dir.join("state.db")).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(home.path().join(".openconductor").join("state.db")).unwrap(), "old");
    }

    #[test]
    fn a_fresh_home_gets_the_new_path_and_nothing_is_created_for_it() {
        let home = tempfile::tempdir().unwrap();
        let dir = migrate_data_dir(home.path());
        assert_eq!(dir, home.path().join(".condurence"));
        assert!(!dir.exists() && !home.path().join(".openconductor").exists());
    }
}
