//! Crash recovery: cleans up what a previous launch left behind.
//!
//! Agents run in their own process groups, so they survive the app being killed or crashing. A
//! graceful exit stops them, but nothing runs on a crash. At the next launch this reaps them the way
//! a process supervisor does: it trusts only a recorded pid whose start time still matches, and only
//! kills a group the session itself led. A pid alone is not enough, because the OS reuses them.

use std::path::Path;
use std::time::Duration;

use crate::error::AppResult;
use crate::pty::process_group;
use crate::runtime::state_db::StateDb;

/// What recovery found and did, for the startup log.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RecoveryReport {
    /// Sessions whose process was still running and was stopped.
    pub reaped: usize,
    /// Sessions whose process had already exited.
    pub already_gone: usize,
    /// Sessions whose pid now belongs to a different process, which was left alone.
    pub pid_reused: usize,
    /// Run directories, with their token-bearing configs, that were removed.
    pub removed_run_dirs: usize,
}

impl RecoveryReport {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Reaps agents left by an earlier launch and removes the files of runs that are over.
///
/// `launched_ms` is when this launch began. Only sessions that started before it are touched, so
/// a session this launch has already started is never mistaken for a leftover. Call it before any
/// run starts. `run_dirs` is the directory that holds one subdirectory per run.
pub fn recover(db: &StateDb, launched_ms: i64, run_dirs: &Path, grace: Duration) -> AppResult<RecoveryReport> {
    let mut report = RecoveryReport::default();
    let mut to_stop = Vec::new();

    for session in db.unfinished_pty_sessions(launched_ms)? {
        match (session.pid, session.started.as_deref()) {
            (Some(pid), Some(recorded)) => match process_group::process_identity(pid) {
                None => report.already_gone += 1,
                Some(current) if current == recorded && process_group::is_group_leader(pid) => to_stop.push(pid),
                Some(_) => report.pid_reused += 1,
            },
            // The launch ended before the process was recorded, so there is nothing to look up.
            _ => report.already_gone += 1,
        }
        db.end_pty_session_unobserved(&session.id, launched_ms)?;
    }

    // Each stop waits out a grace period, so run them side by side.
    report.reaped = to_stop.len();
    let stops: Vec<_> = to_stop
        .into_iter()
        .map(|pid| std::thread::spawn(move || process_group::terminate_tree(pid, grace)))
        .collect();
    for stop in stops {
        let _ = stop.join();
    }

    report.removed_run_dirs = remove_finished_run_dirs(db, run_dirs)?;
    Ok(report)
}

/// Removes the directory of every run the database knows to be over. A directory for an unknown
/// run is left alone, as it may belong to something else.
fn remove_finished_run_dirs(db: &StateDb, run_dirs: &Path) -> AppResult<usize> {
    let Ok(entries) = std::fs::read_dir(run_dirs) else {
        return Ok(0);
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
        let over = match entry.file_name().to_str().map(|name| db.run_status(name)) {
            Some(Ok(Some(status))) => status != "running",
            _ => false,
        };
        if is_dir && over && std::fs::remove_dir_all(entry.path()).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}
