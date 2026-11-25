//! Local telemetry and recovery store (`~/.openconductor/state.db`, WAL mode).
//!
//! Holds registered repositories, orchestration run histories, inter-agent MCP message
//! logs and PTY session records. Topology lives in Git; this database never does.

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use rusqlite::{params, Connection};
use serde::Serialize;
use serde_json::Value;

use crate::error::AppResult;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS repos (
    path TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    last_opened_ms INTEGER NOT NULL,
    archived INTEGER NOT NULL DEFAULT 0,
    added_ms INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS runs (
    id TEXT PRIMARY KEY,
    repo_path TEXT NOT NULL,
    root_node_id TEXT NOT NULL,
    goal TEXT NOT NULL,
    status TEXT NOT NULL,
    started_ms INTEGER NOT NULL,
    ended_ms INTEGER
);
CREATE TABLE IF NOT EXISTS run_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id TEXT NOT NULL,
    node_id TEXT,
    kind TEXT NOT NULL,
    payload TEXT NOT NULL,
    created_ms INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_run_events_run ON run_events(run_id, id);
CREATE TABLE IF NOT EXISTS pty_sessions (
    id TEXT PRIMARY KEY,
    run_id TEXT,
    node_id TEXT,
    cwd TEXT NOT NULL,
    command TEXT NOT NULL,
    started_ms INTEGER NOT NULL,
    ended_ms INTEGER,
    exit_code INTEGER
);
";

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RepoRecord {
    pub path: String,
    pub name: String,
    pub last_opened_ms: i64,
    pub archived: bool,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RunEvent {
    pub id: i64,
    pub run_id: String,
    pub node_id: Option<String>,
    pub kind: String,
    pub payload: Value,
    pub created_ms: i64,
}

pub struct StateDb {
    conn: Mutex<Connection>,
}

impl StateDb {
    pub fn open(path: &Path) -> AppResult<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> AppResult<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> AppResult<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(3))?;
        conn.execute_batch(SCHEMA)?;
        // Databases created before archiving existed lack the column.
        let has_archived = conn
            .prepare("SELECT 1 FROM pragma_table_info('repos') WHERE name = 'archived'")?
            .exists([])?;
        if !has_archived {
            conn.execute("ALTER TABLE repos ADD COLUMN archived INTEGER NOT NULL DEFAULT 0", [])?;
        }
        // The list order is when a repository was first added, which never changes, so opening
        // one does not move it. Existing rows start from their last-opened time, which keeps the
        // order users already see.
        let has_added = conn
            .prepare("SELECT 1 FROM pragma_table_info('repos') WHERE name = 'added_ms'")?
            .exists([])?;
        if !has_added {
            conn.execute("ALTER TABLE repos ADD COLUMN added_ms INTEGER NOT NULL DEFAULT 0", [])?;
            conn.execute("UPDATE repos SET added_ms = last_opened_ms", [])?;
        }
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn upsert_repo(&self, path: &str, name: &str, now_ms: i64) -> AppResult<()> {
        self.conn().execute(
            "INSERT INTO repos (path, name, last_opened_ms, added_ms) VALUES (?1, ?2, ?3, ?3)
             ON CONFLICT(path) DO UPDATE SET name = excluded.name, last_opened_ms = excluded.last_opened_ms, archived = 0",
            params![path, name, now_ms],
        )?;
        Ok(())
    }

    /// Repositories in the sidebar list, newest added first. The order never changes when a
    /// repository is opened. Archived ones are excluded.
    pub fn recent_repos(&self, limit: i64) -> AppResult<Vec<RepoRecord>> {
        self.list_repos(false, limit)
    }

    /// Archived repositories, newest first.
    pub fn archived_repos(&self, limit: i64) -> AppResult<Vec<RepoRecord>> {
        self.list_repos(true, limit)
    }

    fn list_repos(&self, archived: bool, limit: i64) -> AppResult<Vec<RepoRecord>> {
        let conn = self.conn();
        let mut statement = conn.prepare(
            "SELECT path, name, last_opened_ms, archived FROM repos WHERE archived = ?1
             ORDER BY added_ms DESC, path ASC LIMIT ?2",
        )?;
        let rows = statement.query_map(params![archived as i64, limit], |row| {
            Ok(RepoRecord {
                path: row.get(0)?,
                name: row.get(1)?,
                last_opened_ms: row.get(2)?,
                archived: row.get::<_, i64>(3)? != 0,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Hides a repository from Recent (or restores it). The folder on disk is untouched.
    pub fn set_repo_archived(&self, path: &str, archived: bool) -> AppResult<()> {
        self.conn().execute(
            "UPDATE repos SET archived = ?2 WHERE path = ?1",
            params![path, archived as i64],
        )?;
        Ok(())
    }

    /// Forgets a repository entirely. Only the list entry is removed; the folder on disk is untouched.
    pub fn forget_repo(&self, path: &str) -> AppResult<()> {
        self.conn().execute("DELETE FROM repos WHERE path = ?1", params![path])?;
        Ok(())
    }

    pub fn insert_run(&self, run_id: &str, repo_path: &str, root_node_id: &str, goal: &str, started_ms: i64) -> AppResult<()> {
        self.conn().execute(
            "INSERT INTO runs (id, repo_path, root_node_id, goal, status, started_ms) VALUES (?1, ?2, ?3, ?4, 'running', ?5)",
            params![run_id, repo_path, root_node_id, goal, started_ms],
        )?;
        Ok(())
    }

    pub fn finish_run(&self, run_id: &str, status: &str, ended_ms: i64) -> AppResult<()> {
        self.conn().execute(
            "UPDATE runs SET status = ?2, ended_ms = ?3 WHERE id = ?1",
            params![run_id, status, ended_ms],
        )?;
        Ok(())
    }

    pub fn insert_event(&self, run_id: &str, node_id: Option<&str>, kind: &str, payload: &Value, created_ms: i64) -> AppResult<()> {
        self.conn().execute(
            "INSERT INTO run_events (run_id, node_id, kind, payload, created_ms) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![run_id, node_id, kind, payload.to_string(), created_ms],
        )?;
        Ok(())
    }

    pub fn list_events(&self, run_id: &str, limit: i64) -> AppResult<Vec<RunEvent>> {
        let conn = self.conn();
        let mut statement = conn.prepare(
            "SELECT id, run_id, node_id, kind, payload, created_ms FROM run_events
             WHERE run_id = ?1 ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = statement.query_map(params![run_id, limit], |row| {
            let payload: String = row.get(4)?;
            Ok(RunEvent {
                id: row.get(0)?,
                run_id: row.get(1)?,
                node_id: row.get(2)?,
                kind: row.get(3)?,
                payload: serde_json::from_str(&payload).unwrap_or(Value::Null),
                created_ms: row.get(5)?,
            })
        })?;
        let mut events = rows.collect::<Result<Vec<_>, _>>()?;
        events.reverse();
        Ok(events)
    }

    pub fn record_pty_session(
        &self,
        id: &str,
        run_id: Option<&str>,
        node_id: Option<&str>,
        cwd: &Path,
        command: &str,
        started_ms: i64,
    ) -> AppResult<()> {
        self.conn().execute(
            "INSERT INTO pty_sessions (id, run_id, node_id, cwd, command, started_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, run_id, node_id, cwd.to_string_lossy(), command, started_ms],
        )?;
        Ok(())
    }

    pub fn finish_pty_session(&self, id: &str, exit_code: i32, ended_ms: i64) -> AppResult<()> {
        self.conn().execute(
            "UPDATE pty_sessions SET exit_code = ?2, ended_ms = ?3 WHERE id = ?1",
            params![id, exit_code, ended_ms],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn opening_a_repo_does_not_move_it_in_the_list() {
        let db = StateDb::open_in_memory().unwrap();
        db.upsert_repo("/a", "a", 1).unwrap();
        db.upsert_repo("/b", "b", 2).unwrap();
        db.upsert_repo("/c", "c", 3).unwrap();
        let order = |db: &StateDb| db.recent_repos(10).unwrap().into_iter().map(|r| r.path).collect::<Vec<_>>();
        assert_eq!(order(&db), ["/c", "/b", "/a"], "newest added first");

        // Opening the oldest repo later must leave every row where it was.
        db.upsert_repo("/a", "a", 100).unwrap();
        db.upsert_repo("/b", "b", 200).unwrap();
        assert_eq!(order(&db), ["/c", "/b", "/a"]);
        // The open time is still recorded.
        let a = db.recent_repos(10).unwrap().into_iter().find(|r| r.path == "/a").unwrap();
        assert_eq!(a.last_opened_ms, 100);
    }

    #[test]
    fn archiving_and_restoring_returns_a_repo_to_its_original_place() {
        let db = StateDb::open_in_memory().unwrap();
        db.upsert_repo("/a", "a", 1).unwrap();
        db.upsert_repo("/b", "b", 2).unwrap();
        db.upsert_repo("/c", "c", 3).unwrap();
        db.set_repo_archived("/b", true).unwrap();
        db.set_repo_archived("/b", false).unwrap();
        let order: Vec<_> = db.recent_repos(10).unwrap().into_iter().map(|r| r.path).collect();
        assert_eq!(order, ["/c", "/b", "/a"]);
        // Re-opening an archived repo restores it in place, too.
        db.set_repo_archived("/b", true).unwrap();
        db.upsert_repo("/b", "b", 500).unwrap();
        let order: Vec<_> = db.recent_repos(10).unwrap().into_iter().map(|r| r.path).collect();
        assert_eq!(order, ["/c", "/b", "/a"]);
    }

    #[test]
    fn archived_repos_leave_recent_and_return_when_reopened() {
        let db = StateDb::open_in_memory().unwrap();
        db.upsert_repo("/a", "a", 1).unwrap();
        db.upsert_repo("/b", "b", 2).unwrap();
        db.set_repo_archived("/a", true).unwrap();
        assert_eq!(db.recent_repos(10).unwrap().len(), 1);
        let archived = db.archived_repos(10).unwrap();
        assert_eq!(archived.len(), 1);
        assert!(archived[0].archived);

        db.set_repo_archived("/a", false).unwrap();
        assert_eq!(db.recent_repos(10).unwrap().len(), 2);

        db.set_repo_archived("/a", true).unwrap();
        db.upsert_repo("/a", "a", 5).unwrap();
        assert!(db.archived_repos(10).unwrap().is_empty(), "opening an archived repo restores it");
    }

    #[test]
    fn forgotten_repos_disappear_from_both_lists() {
        let db = StateDb::open_in_memory().unwrap();
        db.upsert_repo("/a", "a", 1).unwrap();
        db.upsert_repo("/b", "b", 2).unwrap();
        db.set_repo_archived("/b", true).unwrap();
        db.forget_repo("/a").unwrap();
        db.forget_repo("/b").unwrap();
        assert!(db.recent_repos(10).unwrap().is_empty());
        assert!(db.archived_repos(10).unwrap().is_empty());
    }

    #[test]
    fn legacy_database_without_archived_column_is_migrated() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE repos (path TEXT PRIMARY KEY, name TEXT NOT NULL, last_opened_ms INTEGER NOT NULL);
             INSERT INTO repos VALUES ('/old', 'old', 7);",
        )
        .unwrap();
        let db = StateDb::init(conn).unwrap();
        let recent = db.recent_repos(10).unwrap();
        assert_eq!(recent.len(), 1);
        assert!(!recent[0].archived);
    }

    #[test]
    fn migration_keeps_the_order_users_already_see() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE repos (path TEXT PRIMARY KEY, name TEXT NOT NULL, last_opened_ms INTEGER NOT NULL, archived INTEGER NOT NULL DEFAULT 0);
             INSERT INTO repos (path, name, last_opened_ms) VALUES ('/old', 'old', 10), ('/mid', 'mid', 20), ('/new', 'new', 30);",
        )
        .unwrap();
        let db = StateDb::init(conn).unwrap();
        let order: Vec<_> = db.recent_repos(10).unwrap().into_iter().map(|r| r.path).collect();
        assert_eq!(order, ["/new", "/mid", "/old"]);
        db.upsert_repo("/old", "old", 999).unwrap();
        let order: Vec<_> = db.recent_repos(10).unwrap().into_iter().map(|r| r.path).collect();
        assert_eq!(order, ["/new", "/mid", "/old"], "opening after migration still does not reorder");
    }

    #[test]
    fn run_events_round_trip_in_order() {
        let db = StateDb::open_in_memory().unwrap();
        db.insert_run("r1", "/repo", "ceo", "goal", 1).unwrap();
        db.insert_event("r1", Some("ceo"), "status", &json!({"status": "running"}), 2).unwrap();
        db.insert_event("r1", None, "approval", &json!({"approved": true}), 3).unwrap();
        let events = db.list_events("r1", 10).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].kind, "status");
        assert_eq!(events[1].payload["approved"], true);
        db.finish_run("r1", "stopped", 4).unwrap();
    }
}
