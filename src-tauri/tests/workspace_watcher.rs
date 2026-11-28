//! The debounced watcher must surface edits as `workspace-dirty://{id}` with a live count.

use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use condurence_lib::git::watcher::{workspace_id, WorkspaceWatchers};
use condurence_lib::runtime::events::EventSink;
use serde_json::Value;

#[derive(Default)]
struct RecordingSink {
    events: Mutex<Vec<(String, Value)>>,
}

impl EventSink for RecordingSink {
    fn emit(&self, event: &str, payload: Value) {
        self.events.lock().unwrap().push((event.to_string(), payload));
    }
}

fn git(dir: &std::path::Path, args: &[&str]) {
    assert!(Command::new("git").current_dir(dir).args(args).status().unwrap().success());
}

#[tokio::test(flavor = "multi_thread")]
async fn edits_produce_a_debounced_dirty_count() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "Condurence Test"]);
    std::fs::write(repo.join("README.md"), "# demo\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "init"]);

    let sink = Arc::new(RecordingSink::default());
    let events: Arc<dyn EventSink> = sink.clone();
    let watchers = WorkspaceWatchers::default();
    let id = watchers.watch(events, repo.clone()).expect("watch should start");
    assert_eq!(id, workspace_id(&repo));
    tokio::time::sleep(Duration::from_millis(400)).await; // let the OS watcher arm

    std::fs::write(repo.join("new.txt"), "hello\n").unwrap();
    std::fs::write(repo.join("README.md"), "# demo\nmore\n").unwrap();

    let expected = format!("workspace-dirty://{id}");
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut best = 0;
    while Instant::now() < deadline && best < 2 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        for (name, payload) in sink.events.lock().unwrap().iter() {
            if *name == expected {
                best = best.max(payload["changedFilesCount"].as_u64().unwrap_or(0));
            }
        }
    }
    watchers.unwatch(&repo);
    assert!(best >= 2, "expected a dirty count of 2 (README + new file), saw {best}");
}
