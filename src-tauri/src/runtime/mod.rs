//! Core runtime: owns every long-lived subsystem and coordinates startup and shutdown.

pub mod events;
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
use events::EventSink;
use state_db::{RepoRecord, StateDb};

pub struct CoreRuntime {
    pub events: Arc<dyn EventSink>,
    pub db: Arc<StateDb>,
    pub pty: Arc<PtyManager>,
    pub locks: Arc<RepoLocks>,
    pub watchers: Arc<WorkspaceWatchers>,
    pub router: Arc<Router>,
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
        self.db.recent_repos(20)
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

fn state_db_path() -> AppResult<PathBuf> {
    let home = home_dir().ok_or_else(|| AppError::msg("cannot locate the home directory"))?;
    Ok(home.join(".openconductor").join("state.db"))
}
