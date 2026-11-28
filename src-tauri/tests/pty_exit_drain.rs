//! When a session's exit is published, everything the process printed must already be in its
//! history. The router reads that history right after the exit to build a child's summary, so
//! output still sitting in the PTY at that moment would be silently lost.

#![cfg(unix)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use condurence_lib::pty::{PtyManager, SpawnSpec};
use condurence_lib::runtime::events::EventSink;
use serde_json::Value;

struct NullSink;

impl EventSink for NullSink {
    fn emit(&self, _event: &str, _payload: Value) {}
}

fn knob(name: &str, default: usize) -> usize {
    std::env::var(name).ok().and_then(|value| value.parse().ok()).unwrap_or(default)
}

/// Runs one short-lived process and reports whether its last line was in the history by the
/// time its exit became visible.
async fn last_line_survives(manager: &PtyManager, cwd: &std::path::Path, id: String, lines: usize) -> bool {
    let script = format!(
        "i=0; while [ $i -lt {lines} ]; do echo \"line $i padding padding padding padding padding\"; i=$((i+1)); done; echo FINAL-MARKER; exit 3"
    );
    let session = manager
        .spawn(SpawnSpec {
            session_id: id,
            cwd: cwd.to_path_buf(),
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script],
            env: vec![],
            cols: 120,
            rows: 36,
        })
        .unwrap();

    let mut exit = session.subscribe_exit();
    let code = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let Some(code) = *exit.borrow_and_update() {
                return code;
            }
            exit.changed().await.unwrap();
        }
    })
    .await
    .expect("the process should exit");
    assert_eq!(code, 3);

    // Read the history the instant the exit is visible, as the router does.
    session.history().contains("FINAL-MARKER")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn output_printed_before_exit_is_in_the_history_when_the_exit_is_published() {
    let manager = Arc::new(PtyManager::new(Arc::new(NullSink)));
    let dir = Arc::new(tempfile::tempdir().unwrap());
    let rounds = knob("DRAIN_ROUNDS", 40);
    let parallel = knob("DRAIN_PARALLEL", 8);
    let lines = knob("DRAIN_LINES", 1500);
    let lost = Arc::new(AtomicUsize::new(0));

    for round in 0..rounds {
        let mut tasks = Vec::new();
        for slot in 0..parallel {
            let (manager, dir, lost) = (manager.clone(), dir.clone(), lost.clone());
            tasks.push(tokio::spawn(async move {
                let id = format!("drain-{round}-{slot}");
                if !last_line_survives(&manager, dir.path(), id, lines).await {
                    lost.fetch_add(1, Ordering::SeqCst);
                }
            }));
        }
        for task in tasks {
            task.await.unwrap();
        }
    }

    let total = rounds * parallel;
    let lost = lost.load(Ordering::SeqCst);
    assert_eq!(lost, 0, "{lost} of {total} sessions published their exit before their last output was read");
}
