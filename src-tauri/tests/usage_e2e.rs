//! End-to-end token usage: a real run through the real router, PTY, MCP server and state database,
//! with the provider CLI replaced by a Python double that writes its session log the way Claude
//! Code does. The meter then reads that log through the same functions the app's commands call.
//!
//! Covers: the run meter while the run is live and after it stops, the directory and time
//! attribution that keeps unrelated sessions out, the profile pane's all-sessions summary, and the
//! camelCase shape the frontend reads.

// Runs a Python test double as the provider CLI, so it is Unix-only.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use open_conductor_lib::git::locks::RepoLocks;
use open_conductor_lib::graph::{default_graph, OrchestrationGraph, RelationType};
use open_conductor_lib::mcp::{router::Router, server};
use open_conductor_lib::pty::PtyManager;
use open_conductor_lib::runtime::{events::EventSink, now_ms, state_db::StateDb};
use open_conductor_lib::usage::{self, ProviderUsage, UsageReader};
use serde_json::{json, Value};

const EDGE_ID: &str = "e-ceo-senior-dev";

/// Stand-in for an interactive `claude`. It writes one session log under `$HOME/.claude/projects`,
/// in the directory named after its working directory, then exits. The log has a reply that is
/// repeated across content blocks (must count once), a second reply, and a few non-usage lines.
const FAKE_PROVIDER: &str = r#"#!/usr/bin/env python3
import json, os, sys, uuid
from datetime import datetime, timezone

cwd = os.getcwd()
project = os.path.join(os.environ["HOME"], ".claude", "projects",
                       "".join(c if c.isascii() and c.isalnum() else "-" for c in cwd))
os.makedirs(project, exist_ok=True)

def stamp():
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.%f")[:-3] + "Z"

def reply(request, usage):
    return {"type": "assistant", "requestId": request, "cwd": cwd, "timestamp": stamp(),
            "message": {"id": "msg-" + request, "usage": usage}}

lines = [
    {"type": "user", "cwd": cwd, "timestamp": stamp(), "message": {"role": "user", "content": "hi"}},
    reply("req-1", {"input_tokens": 1000, "output_tokens": 300,
                    "cache_read_input_tokens": 8000, "cache_creation_input_tokens": 50}),
    reply("req-1", {"input_tokens": 1000, "output_tokens": 300,
                    "cache_read_input_tokens": 8000, "cache_creation_input_tokens": 50}),
    {"type": "attachment", "cwd": cwd, "timestamp": stamp(), "attachment": {"type": "file"}},
    reply("req-2", {"input_tokens": 200, "output_tokens": 40,
                    "cache_read_input_tokens": 1000, "cache_creation_input_tokens": 0}),
]
with open(os.path.join(project, str(uuid.uuid4()) + ".jsonl"), "w") as handle:
    for line in lines:
        handle.write(json.dumps(line) + "\n")
sys.exit(0)
"#;

#[derive(Default)]
struct RecordingSink {
    events: Mutex<Vec<(String, Value)>>,
}

impl RecordingSink {
    fn saw_status(&self, run_id: &str, node_id: &str, status: &str) -> bool {
        let wanted = format!("node-status://{node_id}");
        self.events
            .lock()
            .unwrap()
            .iter()
            .any(|(name, payload)| *name == wanted && payload["runId"] == run_id && payload["status"] == status)
    }
}

impl EventSink for RecordingSink {
    fn emit(&self, event: &str, payload: Value) {
        self.events.lock().unwrap().push((event.to_string(), payload));
    }
}

fn build_router(db: Arc<StateDb>, provider: &Path) -> (Arc<Router>, Arc<RecordingSink>) {
    std::env::set_var("OPENCONDUCTOR_CLAUDE_BIN", provider);
    let sink = Arc::new(RecordingSink::default());
    let events: Arc<dyn EventSink> = sink.clone();
    let pty = Arc::new(PtyManager::new(events.clone()));
    let locks = Arc::new(RepoLocks::default());
    let (listener, port) = server::bind_loopback().unwrap();
    let bridge_exe = PathBuf::from(env!("CARGO_BIN_EXE_Condurence"));
    let router = Arc::new(Router::new(events, db, pty, locks, port, bridge_exe));
    server::serve(listener, router.clone()).unwrap();
    (router, sink)
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git").current_dir(dir).args(args).status().unwrap();
    assert!(status.success(), "git {args:?} failed");
}

fn make_repo(parent: &Path, name: &str) -> PathBuf {
    let repo = parent.join(name);
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "Condurence Test"]);
    std::fs::write(repo.join("README.md"), "# demo\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    repo
}

/// The root of a two-node topology, with the delegation edge kept so the graph is realistic.
fn two_node_graph() -> OrchestrationGraph {
    let mut graph = default_graph();
    graph.nodes.retain(|node| node.id == "ceo" || node.id == "senior-dev");
    graph.edges.retain(|edge| edge.id == EDGE_ID);
    let edge = &mut graph.edges[0];
    edge.data.relation_type = RelationType::DelegatesTo;
    graph
}

async fn wait_for(timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    condition()
}

fn row<'a>(rows: &'a [ProviderUsage], provider: &str) -> &'a ProviderUsage {
    rows.iter()
        .find(|row| row.provider == provider)
        .unwrap_or_else(|| panic!("no row for {provider} in {rows:?}"))
}

/// Writes a Claude session log for `cwd` into `home`, the way the fake provider does, with the
/// given start time and one reply. Used for sessions the run did not create.
fn write_claude_session(home: &Path, cwd: &Path, started_ms: i64, input: u64) {
    let encoded: String = cwd
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let project = home.join(".claude").join("projects").join(encoded);
    std::fs::create_dir_all(&project).unwrap();
    let line = json!({
        "type": "assistant",
        "requestId": format!("extra-{input}"),
        "cwd": cwd.to_string_lossy(),
        "timestamp": iso(started_ms),
        "message": { "id": format!("msg-extra-{input}"), "usage": { "input_tokens": input, "output_tokens": 0 } },
    });
    std::fs::write(project.join(format!("extra-{input}.jsonl")), format!("{line}\n")).unwrap();
}

/// ISO-8601 UTC with milliseconds, as the CLIs write it.
fn iso(ms: i64) -> String {
    let days = ms.div_euclid(86_400_000);
    let rest = ms.rem_euclid(86_400_000);
    // Civil date from day count (Howard Hinnant's algorithm), independent of the crate under test.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rest / 3_600_000,
        rest / 60_000 % 60,
        rest / 1_000 % 60,
        rest % 1_000
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn usage_meter_end_to_end() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    // The fake provider writes under $HOME, and the reader is pointed at the same place.
    std::env::set_var("HOME", &home);

    let provider = tmp.path().join("claude");
    std::fs::write(&provider, FAKE_PROVIDER).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&provider, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let db = Arc::new(StateDb::open_in_memory().unwrap());
    let (router, sink) = build_router(db.clone(), &provider);
    let repo = make_repo(tmp.path(), "usage-demo");
    let reader = UsageReader::default();

    // 1. A run starts, its root CLI writes a session log and exits.
    let run_id = router
        .start_run(repo.clone(), two_node_graph(), "ceo".into(), "measure my usage".into())
        .await
        .expect("run should start");
    let finished = wait_for(Duration::from_secs(30), || sink.saw_status(&run_id, "ceo", "idle")).await;
    assert!(finished, "the root CLI never finished");

    // 2. While the run is live, the run meter sees the session with duplicates counted once.
    let live = db.run_usage_inputs(&run_id).unwrap().expect("the run is recorded");
    assert_eq!(live.ended_ms, None, "a live run has no end time yet");
    assert_eq!(live.commands, ["claude"]);
    let repo_path = repo.to_string_lossy().into_owned();
    assert!(live.cwds.contains(&repo_path), "the root's directory is recorded: {:?}", live.cwds);

    let rows = usage::measure_run(&reader, &home, &live, now_ms());
    assert_eq!(rows.len(), 1, "only the provider that ran appears");
    let claude = row(&rows, "claude");
    assert!(claude.measured);
    assert_eq!(claude.sessions, 1);
    // req-1 appears twice in the log but counts once: output is 300 + 40, not 300 + 300 + 40.
    assert_eq!(claude.input_tokens, 1_200);
    assert_eq!(claude.output_tokens, 340);
    assert_eq!(claude.cache_read_tokens, 9_000);
    assert_eq!(claude.cache_write_tokens, 50);

    // 3. Another session in an unrelated directory, started during the run, is left out of the
    //    run meter but is counted by the profile pane.
    let unrelated = tmp.path().join("unrelated-repo");
    std::fs::create_dir_all(&unrelated).unwrap();
    write_claude_session(&home, &unrelated, live.started_ms + 1_000, 5_000);
    let during = usage::measure_run(&reader, &home, &live, now_ms());
    assert_eq!(row(&during, "claude").input_tokens, 1_200, "the unrelated session must not leak into the run");

    // 4. The run stops. Its totals hold, and a session that starts after it ended is excluded.
    router.stop_run(&run_id).await.expect("stop should succeed");
    let stopped = db.run_usage_inputs(&run_id).unwrap().expect("the run is recorded");
    let ended = stopped.ended_ms.expect("a stopped run has an end time");
    // Start the late session in real time, after the stop and its slack, so it is not dated in the
    // future relative to the queries below.
    tokio::time::sleep(Duration::from_secs(3)).await;
    let late_start = now_ms();
    assert!(late_start > ended + 2_000, "the late session must start after the run's slack");
    write_claude_session(&home, &repo, late_start, 777);
    let after = usage::measure_run(&reader, &home, &stopped, now_ms());
    let claude_after = row(&after, "claude");
    assert_eq!(claude_after.sessions, 1, "a session started after the run ended is not part of it");
    assert_eq!(claude_after.input_tokens, 1_200);

    // 5. The profile pane's summary counts every session started in the last 30 days.
    let summary = usage::summarize(&reader, &home, usage::SUMMARY_DAYS, now_ms());
    assert_eq!(summary.iter().map(|row| row.provider.as_str()).collect::<Vec<_>>(), ["claude", "codex", "opencode", "gemini"]);
    let all_claude = row(&summary, "claude");
    assert_eq!(all_claude.sessions, 3, "the run's session, the unrelated one and the late one");
    assert_eq!(all_claude.input_tokens, 1_200 + 5_000 + 777);
    assert_eq!(all_claude.output_tokens, 340);
    assert!(row(&summary, "codex").measured && row(&summary, "codex").sessions == 0);
    assert!(!row(&summary, "opencode").measured, "opencode is listed but not measured yet");
    assert!(!row(&summary, "gemini").measured, "gemini is listed but not measured yet");

    // 6. The wire shape matches what the frontend reads.
    let wire = serde_json::to_value(&summary[0]).unwrap();
    for key in ["provider", "measured", "sessions", "inputTokens", "outputTokens", "cacheReadTokens", "cacheWriteTokens"] {
        assert!(wire.get(key).is_some(), "missing {key} in {wire}");
    }
}
