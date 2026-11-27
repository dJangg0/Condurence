//! What happens when a terminal session closes: its leftover processes are stopped, its OS handles
//! are released, its output stays replayable for a bounded time, and old sessions age out.

#![cfg(unix)]

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use condurence_lib::pty::{PtyManager, PtySession, SpawnSpec};
use condurence_lib::runtime::events::EventSink;
use serde_json::Value;

struct NullSink;

impl EventSink for NullSink {
    fn emit(&self, _event: &str, _payload: Value) {}
}

fn run_shell(manager: &PtyManager, cwd: &Path, id: &str, script: &str) -> Arc<PtySession> {
    manager
        .spawn(SpawnSpec {
            session_id: id.into(),
            cwd: cwd.to_path_buf(),
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
            env: vec![],
            cols: 80,
            rows: 24,
        })
        .unwrap()
}

async fn wait_until(timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    condition()
}

fn alive(pid: i32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_process_left_behind_by_the_session_is_stopped_when_it_closes() {
    let manager = PtyManager::new(Arc::new(NullSink));
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("straggler.pid");

    // The leader backgrounds a long process and exits at once, as `nohup npm run dev &` would. The
    // process ignores SIGHUP, because the hangup the kernel sends when a terminal's leader exits
    // already ends ordinary background jobs. Only a job that survives it needs sweeping.
    let script = format!("trap '' HUP; sleep 300 & echo $! > {}; exit 0", pid_file.display());
    let session = run_shell(&manager, dir.path(), "straggler", &script);

    assert!(wait_until(Duration::from_secs(5), || pid_file.exists()).await, "the straggler should start");
    let straggler: i32 = std::fs::read_to_string(&pid_file).unwrap().trim().parse().unwrap();

    assert!(wait_until(Duration::from_secs(10), || session.is_closed()).await, "the session should close");
    assert!(
        wait_until(Duration::from_secs(5), || !alive(straggler)).await,
        "process {straggler} outlived the session that started it"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_closed_session_releases_its_handles_but_keeps_its_output() {
    let manager = PtyManager::new(Arc::new(NullSink));
    let dir = tempfile::tempdir().unwrap();
    let session = run_shell(&manager, dir.path(), "closed", "echo remembered-output; exit 0");

    assert!(wait_until(Duration::from_secs(10), || session.is_closed()).await, "handles should be released");
    assert!(session.history().contains("remembered-output"), "output must outlive the handles");
    assert!(session.write(b"x").is_err(), "writing to a closed session must fail, not hang");
    assert!(session.resize(100, 30).is_ok(), "a view may resize after the process ended");
    assert!(manager.get("closed").is_ok(), "a finished session stays replayable");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_the_most_recent_finished_sessions_are_kept() {
    let manager = PtyManager::with_retention(Arc::new(NullSink), 2);
    let dir = tempfile::tempdir().unwrap();

    for index in 0..4 {
        let id = format!("s{index}");
        let session = run_shell(&manager, dir.path(), &id, &format!("echo output-{index}; exit 0"));
        assert!(wait_until(Duration::from_secs(10), || session.is_closed()).await);
        // Retirement follows the handle release, so give it a moment before the next one.
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    assert!(manager.get("s0").is_err(), "the oldest finished session should have been released");
    assert!(manager.get("s1").is_err());
    assert!(manager.get("s2").unwrap().history().contains("output-2"));
    assert!(manager.get("s3").unwrap().history().contains("output-3"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn release_drops_a_finished_session_and_never_a_running_one() {
    let manager = PtyManager::new(Arc::new(NullSink));
    let dir = tempfile::tempdir().unwrap();

    let running = run_shell(&manager, dir.path(), "running", "sleep 30");
    assert!(!manager.release("running"), "a running session must not be released");
    assert!(manager.get("running").is_ok());

    let done = run_shell(&manager, dir.path(), "done", "exit 0");
    assert!(wait_until(Duration::from_secs(10), || done.is_closed()).await);
    assert!(manager.release("done"));
    assert!(manager.get("done").is_err());
    assert!(!manager.release("done"), "releasing twice is harmless");

    let pty = manager.get("running").unwrap();
    assert!(pty.is_running());
    manager.terminate("running");
    drop(running);
}
