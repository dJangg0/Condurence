//! Local telemetry and recovery store (`~/.condurence/state.db`, WAL mode).
//!
//! Holds registered repositories, orchestration run histories, inter-agent MCP message
//! logs and PTY session records. Topology lives in Git; this database never does.

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use rusqlite::{params, Connection, OptionalExtension};
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
CREATE TABLE IF NOT EXISTS repo_settings (
    repo_path TEXT PRIMARY KEY,
    json TEXT NOT NULL
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

/// What the usage meter needs to attribute a run's sessions. Read back from the database, so a
/// run that finished in an earlier launch can still be measured.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunUsageInputs {
    pub repo_path: String,
    pub started_ms: i64,
    /// Unset while the run is still active.
    pub ended_ms: Option<i64>,
    /// CLI binary names that ran in the run, such as `claude`.
    pub commands: Vec<String>,
    /// Working directories of the run's sessions, the repository root included.
    pub cwds: Vec<String>,
}

/// One orchestration run, as listed in the run history.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RunRecord {
    pub id: String,
    pub repo_path: String,
    pub root_node_id: String,
    pub goal: String,
    /// `running` while live. Then `finished` once the root exits, `stopped` when the user stops
    /// it, or `interrupted` if the app quit while it was running.
    pub status: String,
    pub started_ms: i64,
    pub ended_ms: Option<i64>,
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
        // Databases created before crash recovery lack the process identity of a session.
        let has_pid = conn
            .prepare("SELECT 1 FROM pragma_table_info('pty_sessions') WHERE name = 'pid'")?
            .exists([])?;
        if !has_pid {
            conn.execute("ALTER TABLE pty_sessions ADD COLUMN pid INTEGER", [])?;
            conn.execute("ALTER TABLE pty_sessions ADD COLUMN pid_started TEXT", [])?;
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

    /// Runs started from `repo_path`, newest first.
    pub fn list_runs(&self, repo_path: &str, limit: i64) -> AppResult<Vec<RunRecord>> {
        let conn = self.conn();
        let mut statement = conn.prepare(
            "SELECT id, repo_path, root_node_id, goal, status, started_ms, ended_ms FROM runs
             WHERE repo_path = ?1 ORDER BY started_ms DESC, id DESC LIMIT ?2",
        )?;
        let rows = statement.query_map(params![repo_path, limit], |row| {
            Ok(RunRecord {
                id: row.get(0)?,
                repo_path: row.get(1)?,
                root_node_id: row.get(2)?,
                goal: row.get(3)?,
                status: row.get(4)?,
                started_ms: row.get(5)?,
                ended_ms: row.get(6)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// The stored Git settings of a repository, as JSON. `None` until they are first saved.
    pub fn repo_settings_json(&self, repo_path: &str) -> AppResult<Option<String>> {
        let stored = self
            .conn()
            .query_row("SELECT json FROM repo_settings WHERE repo_path = ?1", params![repo_path], |row| row.get(0))
            .optional()?;
        Ok(stored)
    }

    pub fn put_repo_settings_json(&self, repo_path: &str, json: &str) -> AppResult<()> {
        self.conn().execute(
            "INSERT INTO repo_settings (repo_path, json) VALUES (?1, ?2)
             ON CONFLICT(repo_path) DO UPDATE SET json = excluded.json",
            params![repo_path, json],
        )?;
        Ok(())
    }

    /// Marks runs that were still `running` when the app last quit as `interrupted`. Called at
    /// startup, when no run can be live, so every `running` row is stale.
    pub fn close_interrupted_runs(&self, now_ms: i64) -> AppResult<usize> {
        let changed = self.conn().execute(
            "UPDATE runs SET status = 'interrupted', ended_ms = ?1 WHERE status = 'running'",
            params![now_ms],
        )?;
        Ok(changed)
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

    /// The facts the usage meter needs for one run, or `None` if the run is unknown.
    pub fn run_usage_inputs(&self, run_id: &str) -> AppResult<Option<RunUsageInputs>> {
        let conn = self.conn();
        let run = conn
            .query_row(
                "SELECT repo_path, started_ms, ended_ms FROM runs WHERE id = ?1",
                params![run_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, Option<i64>>(2)?)),
            )
            .optional()?;
        let Some((repo_path, started_ms, ended_ms)) = run else {
            return Ok(None);
        };
        let commands = distinct_strings(
            &conn,
            "SELECT DISTINCT command FROM pty_sessions WHERE run_id = ?1 ORDER BY command",
            run_id,
        )?;
        let mut cwds = distinct_strings(&conn, "SELECT DISTINCT cwd FROM pty_sessions WHERE run_id = ?1", run_id)?;
        if !cwds.contains(&repo_path) {
            cwds.push(repo_path.clone());
        }
        Ok(Some(RunUsageInputs {
            repo_path,
            started_ms,
            ended_ms,
            commands,
            cwds,
        }))
    }

    /// Stores who a session's process is: its pid and the moment it started. A pid alone can be
    /// reused by an unrelated process after the original dies, so recovery trusts only the pair.
    pub fn record_pty_process(&self, id: &str, pid: u32, started: &str) -> AppResult<()> {
        self.conn().execute(
            "UPDATE pty_sessions SET pid = ?2, pid_started = ?3 WHERE id = ?1",
            params![id, pid, started],
        )?;
        Ok(())
    }

    /// Sessions started before `before_ms` that never recorded an end, with the process identity
    /// they recorded, if any. After a crash these are the processes that may still be running.
    pub fn unfinished_pty_sessions(&self, before_ms: i64) -> AppResult<Vec<UnfinishedSession>> {
        let conn = self.conn();
        let mut statement = conn.prepare(
            "SELECT id, pid, pid_started FROM pty_sessions WHERE ended_ms IS NULL AND started_ms <= ?1",
        )?;
        let rows = statement.query_map(params![before_ms], |row| {
            Ok(UnfinishedSession {
                id: row.get(0)?,
                pid: row.get(1)?,
                started: row.get(2)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Closes a session that ended without anyone seeing its exit code, such as one cut off by a
    /// crash.
    pub fn end_pty_session_unobserved(&self, id: &str, ended_ms: i64) -> AppResult<()> {
        self.conn().execute(
            "UPDATE pty_sessions SET ended_ms = ?2 WHERE id = ?1 AND ended_ms IS NULL",
            params![id, ended_ms],
        )?;
        Ok(())
    }

    /// The recorded status of a run, or `None` if it is unknown.
    pub fn run_status(&self, run_id: &str) -> AppResult<Option<String>> {
        Ok(self
            .conn()
            .query_row("SELECT status FROM runs WHERE id = ?1", params![run_id], |row| row.get(0))
            .optional()?)
    }

    /// How many of a run's terminal sessions have no recorded end. Zero once the run is over.
    pub fn open_pty_sessions(&self, run_id: &str) -> AppResult<i64> {
        Ok(self.conn().query_row(
            "SELECT COUNT(*) FROM pty_sessions WHERE run_id = ?1 AND ended_ms IS NULL",
            params![run_id],
            |row| row.get(0),
        )?)
    }

    pub fn finish_pty_session(&self, id: &str, exit_code: i32, ended_ms: i64) -> AppResult<()> {
        self.conn().execute(
            "UPDATE pty_sessions SET exit_code = ?2, ended_ms = ?3 WHERE id = ?1",
            params![id, exit_code, ended_ms],
        )?;
        Ok(())
    }
}

/// A terminal session with no recorded end and the identity of the process it started.
#[derive(Clone, Debug, PartialEq)]
pub struct UnfinishedSession {
    pub id: String,
    pub pid: Option<u32>,
    pub started: Option<String>,
}

fn distinct_strings(conn: &Connection, sql: &str, run_id: &str) -> AppResult<Vec<String>> {
    let mut statement = conn.prepare(sql)?;
    let rows = statement.query_map(params![run_id], |row| row.get::<_, String>(0))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_database_from_before_crash_recovery_gains_the_process_columns_and_keeps_its_rows() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE pty_sessions (
                id TEXT PRIMARY KEY, run_id TEXT, node_id TEXT, cwd TEXT NOT NULL,
                command TEXT NOT NULL, started_ms INTEGER NOT NULL, ended_ms INTEGER, exit_code INTEGER
            );
            INSERT INTO pty_sessions (id, cwd, command, started_ms) VALUES ('old', '/tmp', 'claude', 5);",
        )
        .unwrap();

        let db = StateDb::init(conn).unwrap();

        let rows = db.unfinished_pty_sessions(10).unwrap();
        assert_eq!(rows, vec![UnfinishedSession { id: "old".into(), pid: None, started: None }]);
        db.record_pty_process("old", 4242, "Thu Nov 26 17:25:12 2026").unwrap();
        assert_eq!(db.unfinished_pty_sessions(10).unwrap()[0].pid, Some(4242));
        // Opening it again must not try to add the columns twice.
        let reopened = StateDb::init(Connection::open_in_memory().unwrap());
        assert!(reopened.is_ok());
    }

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
    fn usage_inputs_list_the_providers_and_directories_a_run_used() {
        let db = StateDb::open_in_memory().unwrap();
        db.insert_run("r1", "/repo", "ceo", "goal", 10).unwrap();
        db.record_pty_session("s1", Some("r1"), Some("ceo"), Path::new("/repo"), "claude", 11).unwrap();
        db.record_pty_session("s2", Some("r1"), Some("dev"), Path::new("/repo/.worktrees/a"), "codex", 12).unwrap();
        db.record_pty_session("s3", Some("r1"), Some("dev"), Path::new("/repo/.worktrees/a"), "codex", 13).unwrap();
        db.record_pty_session("s4", None, None, Path::new("/elsewhere"), "claude", 14).unwrap();

        let inputs = db.run_usage_inputs("r1").unwrap().unwrap();
        assert_eq!(inputs.repo_path, "/repo");
        assert_eq!(inputs.started_ms, 10);
        assert_eq!(inputs.ended_ms, None, "an active run has no end yet");
        assert_eq!(inputs.commands, ["claude", "codex"]);
        assert_eq!(inputs.cwds, ["/repo", "/repo/.worktrees/a"]);

        db.finish_run("r1", "stopped", 99).unwrap();
        assert_eq!(db.run_usage_inputs("r1").unwrap().unwrap().ended_ms, Some(99));
        assert!(db.run_usage_inputs("missing").unwrap().is_none());
    }

    #[test]
    fn run_history_is_scoped_to_the_repo_newest_first_and_limited() {
        let db = StateDb::open_in_memory().unwrap();
        db.insert_run("old", "/repo", "ceo", "first goal", 10).unwrap();
        db.insert_run("other", "/elsewhere", "ceo", "not this repo", 20).unwrap();
        db.insert_run("new", "/repo", "ceo", "second goal", 30).unwrap();
        db.finish_run("old", "finished", 15).unwrap();

        let history = db.list_runs("/repo", 10).unwrap();
        assert_eq!(history.iter().map(|run| run.id.as_str()).collect::<Vec<_>>(), ["new", "old"]);
        assert_eq!(history[0].status, "running");
        assert_eq!(history[0].goal, "second goal");
        assert_eq!(history[1].status, "finished");
        assert_eq!(history[1].ended_ms, Some(15));
        assert_eq!(db.list_runs("/repo", 1).unwrap().len(), 1);
        assert!(db.list_runs("/nowhere", 10).unwrap().is_empty());
    }

    #[test]
    fn runs_left_running_by_an_earlier_launch_are_marked_interrupted() {
        let db = StateDb::open_in_memory().unwrap();
        db.insert_run("crashed", "/repo", "ceo", "goal", 10).unwrap();
        db.insert_run("done", "/repo", "ceo", "goal", 20).unwrap();
        db.finish_run("done", "stopped", 25).unwrap();

        assert_eq!(db.close_interrupted_runs(99).unwrap(), 1);
        let history = db.list_runs("/repo", 10).unwrap();
        let crashed = history.iter().find(|run| run.id == "crashed").unwrap();
        assert_eq!(crashed.status, "interrupted");
        assert_eq!(crashed.ended_ms, Some(99));
        let done = history.iter().find(|run| run.id == "done").unwrap();
        assert_eq!(done.status, "stopped", "finished runs keep their status");
        assert_eq!(db.close_interrupted_runs(100).unwrap(), 0);
    }

    #[test]
    fn repo_settings_are_stored_per_repository_and_overwritten() {
        let db = StateDb::open_in_memory().unwrap();
        assert_eq!(db.repo_settings_json("/repo").unwrap(), None);
        db.put_repo_settings_json("/repo", r#"{"remote":"origin"}"#).unwrap();
        db.put_repo_settings_json("/other", r#"{"remote":"upstream"}"#).unwrap();
        db.put_repo_settings_json("/repo", r#"{"remote":"fork"}"#).unwrap();
        assert_eq!(db.repo_settings_json("/repo").unwrap().as_deref(), Some(r#"{"remote":"fork"}"#));
        assert_eq!(db.repo_settings_json("/other").unwrap().as_deref(), Some(r#"{"remote":"upstream"}"#));
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
