//! After a crash the agents of the last launch may still be running. Recovery must stop exactly
//! those, and must never kill a process that merely has the same pid.

#![cfg(unix)]

use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use condurence_lib::pty::process_group::process_identity;
use condurence_lib::runtime::recovery::{recover, RecoveryReport};
use condurence_lib::runtime::state_db::StateDb;

const LAUNCHED_MS: i64 = 2_000;
const GRACE: Duration = Duration::from_millis(500);

/// A process that stands in for an agent the crashed app left running.
fn leftover(own_group: bool) -> Child {
    let mut command = Command::new("sleep");
    command.arg("300").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    if own_group {
        // A PTY session's process leads its own group, so this is what a real agent looks like.
        command.process_group(0);
    }
    command.spawn().unwrap()
}

fn record(db: &StateDb, id: &str, child: &Child, started_ms: i64, identity: Option<String>) {
    db.record_pty_session(id, Some("run"), Some("node"), Path::new("/tmp"), "claude", started_ms).unwrap();
    let started = identity.or_else(|| process_identity(child.id())).expect("the process should be running");
    db.record_pty_process(id, child.id(), &started).unwrap();
}

/// Waits for `child` to end and returns the signal that ended it, if it was signalled.
fn ended_by_signal(child: &mut Child) -> Option<i32> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait().unwrap() {
            return status.signal();
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    None
}

fn still_running(child: &mut Child) -> bool {
    child.try_wait().unwrap().is_none()
}

fn recover_in(db: &StateDb, dir: &Path) -> RecoveryReport {
    recover(db, LAUNCHED_MS, dir, GRACE).unwrap()
}

#[test]
fn an_agent_left_running_by_a_crash_is_stopped_and_its_session_closed() {
    let db = StateDb::open_in_memory().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut agent = leftover(true);
    record(&db, "s1", &agent, 1_000, None);

    let report = recover_in(&db, dir.path());

    assert_eq!(report.reaped, 1, "{report:?}");
    assert!(ended_by_signal(&mut agent).is_some(), "the leftover agent should have been stopped");
    assert!(db.unfinished_pty_sessions(i64::MAX).unwrap().is_empty(), "the session should be closed");
}

#[test]
fn a_reused_pid_is_never_killed() {
    let db = StateDb::open_in_memory().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut stranger = leftover(true);
    // The session recorded a different start time for this pid, as if the pid were later reused.
    record(&db, "s1", &stranger, 1_000, Some("Mon Jan  1 00:00:00 2001".into()));

    let report = recover_in(&db, dir.path());

    assert_eq!((report.reaped, report.pid_reused), (0, 1), "{report:?}");
    assert!(still_running(&mut stranger), "an unrelated process with the same pid must survive");
    assert!(db.unfinished_pty_sessions(i64::MAX).unwrap().is_empty(), "the stale record is still closed");
    stranger.kill().unwrap();
    stranger.wait().unwrap();
}

#[test]
fn a_process_that_is_not_a_group_leader_is_left_alone() {
    let db = StateDb::open_in_memory().unwrap();
    let dir = tempfile::tempdir().unwrap();
    // Same pid and start time, but it shares this test's own group, so killing "its" group would
    // kill the test runner. Only a group the session led is safe to kill.
    let mut member = leftover(false);
    record(&db, "s1", &member, 1_000, None);

    let report = recover_in(&db, dir.path());

    assert_eq!((report.reaped, report.pid_reused), (0, 1), "{report:?}");
    assert!(still_running(&mut member));
    member.kill().unwrap();
    member.wait().unwrap();
}

#[test]
fn a_session_that_already_ended_is_only_closed() {
    let db = StateDb::open_in_memory().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut agent = leftover(true);
    record(&db, "s1", &agent, 1_000, None);
    agent.kill().unwrap();
    agent.wait().unwrap();

    let report = recover_in(&db, dir.path());

    assert_eq!((report.reaped, report.already_gone), (0, 1), "{report:?}");
    assert!(db.unfinished_pty_sessions(i64::MAX).unwrap().is_empty());
}

#[test]
fn a_session_started_after_this_launch_began_is_not_a_leftover() {
    let db = StateDb::open_in_memory().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut fresh = leftover(true);
    record(&db, "s1", &fresh, LAUNCHED_MS + 1_000, None);

    let report = recover_in(&db, dir.path());

    assert!(report.is_empty(), "{report:?}");
    assert!(still_running(&mut fresh), "this launch's own agent must not be stopped");
    assert_eq!(db.unfinished_pty_sessions(i64::MAX).unwrap().len(), 1);
    fresh.kill().unwrap();
    fresh.wait().unwrap();
}

#[test]
fn a_session_that_never_recorded_a_process_is_closed_without_killing_anything() {
    let db = StateDb::open_in_memory().unwrap();
    let dir = tempfile::tempdir().unwrap();
    db.record_pty_session("s1", Some("run"), Some("node"), Path::new("/tmp"), "claude", 1_000).unwrap();

    let report = recover_in(&db, dir.path());

    assert_eq!((report.reaped, report.already_gone), (0, 1), "{report:?}");
    assert!(db.unfinished_pty_sessions(i64::MAX).unwrap().is_empty());
}

#[test]
fn run_directories_of_finished_runs_are_removed_and_other_directories_are_kept() {
    let db = StateDb::open_in_memory().unwrap();
    let root = tempfile::tempdir().unwrap();
    for name in ["over", "running", "unknown"] {
        let dir = root.path().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("n.1.claude-mcp.json"), "{\"token\":\"t\"}").unwrap();
    }
    db.insert_run("over", "/repo", "ceo", "goal", 1).unwrap();
    db.finish_run("over", "interrupted", 2).unwrap();
    db.insert_run("running", "/repo", "ceo", "goal", 1).unwrap();

    let report = recover_in(&db, root.path());

    assert_eq!(report.removed_run_dirs, 1, "{report:?}");
    assert!(!root.path().join("over").exists(), "a finished run's token files should be gone");
    assert!(root.path().join("running").exists(), "a run that is still live keeps its files");
    assert!(root.path().join("unknown").exists(), "a directory the database does not know is not ours to delete");
}
