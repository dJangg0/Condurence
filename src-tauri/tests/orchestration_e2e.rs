//! End-to-end orchestration: a real PTY, the real loopback MCP server, the real router and
//! the real state database. Only the provider CLI is replaced, by a Python test double that
//! speaks MCP over HTTP exactly as `claude` would through the stdio bridge.
//!
//! Covers: delegation and structured result return, the isolated child worktree, the
//! maxRetries budget, the approval gate (approve and reject), and token enforcement.

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
use open_conductor_lib::runtime::{events::EventSink, state_db::StateDb};
use serde_json::{json, Value};

const EDGE_ID: &str = "e-ceo-senior-dev";

/// Stand-in for `claude`. With `-p` it is a delegated child: it edits the worktree, then
/// calls `report_task_result`. Otherwise it is the root orchestrator: it delegates
/// `DELEGATE=<n>` times (default 1), records every tool result, and exits.
const FAKE_PROVIDER: &str = r#"#!/usr/bin/env python3
import json, re, subprocess, sys

argv = sys.argv[1:]
headless = "-p" in argv
config = json.load(open(argv[argv.index("--mcp-config") + 1]))
server = config["mcpServers"]["openconductor"]

# Like claude: launch the MCP server as a stdio child and speak newline-delimited JSON-RPC.
bridge = subprocess.Popen([server["command"]] + server["args"], stdin=subprocess.PIPE,
                          stdout=subprocess.PIPE, text=True)
counter = [0]

def rpc(method, params=None, notify=False):
    message = {"jsonrpc": "2.0", "method": method, "params": params or {}}
    if not notify:
        counter[0] += 1
        message["id"] = counter[0]
    bridge.stdin.write(json.dumps(message) + "\n")
    bridge.stdin.flush()
    if notify:
        return None
    return json.loads(bridge.stdout.readline())

rpc("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "fake-claude", "version": "0"}})
rpc("notifications/initialized", notify=True)

def call(name, arguments):
    reply = rpc("tools/call", {"name": name, "arguments": arguments})
    return reply.get("result", reply.get("error"))

if headless:
    with open("README.md", "a") as handle:
        handle.write("\ngreeting: hello, world\n")
    call("report_task_result", {"status": "completed", "summary": "Added greeting line to README.md"})
    print(json.dumps({"type": "result", "subtype": "success", "result": "Added greeting line to README.md"}))
    sys.exit(0)

prompt = argv[0]
match = re.search(r"DELEGATE=(\d+)", prompt)
count = int(match.group(1)) if match else 1
tools = rpc("tools/list")["result"]["tools"]
results = []
for index in range(count):
    task = "Add a greeting line to README.md" if index == 0 else "again"
    results.append(call("delegate_to_senior-dev", {"task": task, "acceptance_criteria": "README contains greeting"}))
with open("orchestrator-log.json", "w") as handle:
    json.dump({"tools": [tool["name"] for tool in tools], "results": results}, handle)
sys.exit(0)
"#;

#[derive(Default)]
struct RecordingSink {
    events: Mutex<Vec<(String, Value)>>,
}

impl RecordingSink {
    /// Whether `node_id` reached `status` within one specific run. Scoping by run id keeps
    /// one scenario's events from satisfying another scenario's wait.
    fn saw_status(&self, run_id: &str, node_id: &str, status: &str) -> bool {
        let wanted = format!("node-status://{node_id}");
        self.events.lock().unwrap().iter().any(|(name, payload)| {
            *name == wanted && payload["runId"] == run_id && payload["status"] == status
        })
    }
}

impl EventSink for RecordingSink {
    fn emit(&self, event: &str, payload: Value) {
        self.events.lock().unwrap().push((event.to_string(), payload));
    }
}

struct Harness {
    router: Arc<Router>,
    sink: Arc<RecordingSink>,
}

fn build_harness(db: Arc<StateDb>, provider: &Path) -> Harness {
    std::env::set_var("OPENCONDUCTOR_CLAUDE_BIN", provider);
    let sink = Arc::new(RecordingSink::default());
    let events: Arc<dyn EventSink> = sink.clone();
    let pty = Arc::new(PtyManager::new(events.clone()));
    let locks = Arc::new(RepoLocks::default());
    let (listener, port) = server::bind_loopback().unwrap();
    // The real binary serves --mcp-bridge, exactly as it does for an installed app.
    let bridge_exe = PathBuf::from(env!("CARGO_BIN_EXE_Condurence"));
    let router = Arc::new(Router::new(events, db, pty, locks, port, bridge_exe));
    server::serve(listener, router.clone()).unwrap();
    Harness { router, sink }
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
    git(&repo, &["config", "user.name", "OpenConductor Test"]);
    std::fs::write(repo.join("README.md"), "# demo\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    repo
}

/// Root `ceo` delegates to `senior-dev` (isolated) over one edge.
fn two_node_graph(max_retries: u32, requires_approval: bool) -> OrchestrationGraph {
    let mut graph = default_graph();
    graph.nodes.retain(|node| node.id == "ceo" || node.id == "senior-dev");
    graph.edges.retain(|edge| edge.id == EDGE_ID);
    let edge = &mut graph.edges[0];
    edge.data.relation_type = RelationType::DelegatesTo;
    edge.data.max_retries = max_retries;
    edge.data.requires_approval = requires_approval;
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

/// Parses one MCP tool result into `(isError, parsed JSON text when available, raw text)`.
fn tool_result(result: &Value) -> (bool, Value, String) {
    let is_error = result["isError"].as_bool().unwrap_or(false);
    let text = result["content"][0]["text"].as_str().unwrap_or_default().to_string();
    let parsed = serde_json::from_str(&text).unwrap_or(Value::Null);
    (is_error, parsed, text)
}

struct ScenarioOutcome {
    run_id: String,
    log: Value,
    repo: PathBuf,
}

async fn run_scenario(
    harness: &Harness,
    base: &Path,
    name: &str,
    goal: &str,
    max_retries: u32,
    requires_approval: bool,
    approve: Option<bool>,
) -> ScenarioOutcome {
    let repo = make_repo(base, name);
    let run_id = harness
        .router
        .start_run(repo.clone(), two_node_graph(max_retries, requires_approval), "ceo".into(), goal.into())
        .await
        .expect("run should start");

    if let Some(decision) = approve {
        let sink = harness.sink.clone();
        let scoped_run = run_id.clone();
        let waiting = wait_for(Duration::from_secs(30), move || {
            sink.saw_status(&scoped_run, "senior-dev", "waiting_approval")
        })
        .await;
        assert!(waiting, "senior-dev should pause on the approval gate");
        harness
            .router
            .resolve_approval(&run_id, EDGE_ID, decision)
            .expect("approval should resolve");
    }

    let log_path = repo.join("orchestrator-log.json");
    let finished = wait_for(Duration::from_secs(60), || log_path.exists()).await;
    assert!(finished, "root orchestrator never finished ({name})");
    let log: Value = serde_json::from_str(&std::fs::read_to_string(&log_path).unwrap()).unwrap();

    harness.router.stop_run(&run_id).await.expect("stop should succeed");
    ScenarioOutcome { run_id, log, repo }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn orchestration_end_to_end() {
    let tmp = tempfile::tempdir().unwrap();
    let provider = tmp.path().join("claude");
    std::fs::write(&provider, FAKE_PROVIDER).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&provider, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let db = Arc::new(StateDb::open_in_memory().unwrap());
    let harness = build_harness(db.clone(), &provider);
    let base = tmp.path();

    // Scenario 1: delegate, child reports, then the second attempt hits the maxRetries=1 budget.
    let outcome = run_scenario(&harness, base, "budget", "DELEGATE=2 add a greeting", 1, false, None).await;
    let tools = outcome.log["tools"].as_array().unwrap();
    assert_eq!(tools, &vec![json!("delegate_to_senior-dev")], "root must not see report_task_result");

    let (first_error, first, first_text) = tool_result(&outcome.log["results"][0]);
    assert!(!first_error, "first delegation failed: {first_text}");
    assert_eq!(first["node"], "senior-dev");
    assert_eq!(first["status"], "completed");
    assert_eq!(first["exitCode"], 0);
    assert_eq!(first["summary"], "Added greeting line to README.md");
    assert!(first["diffStat"].as_str().unwrap().contains("README.md"));

    // The child ran in an isolated worktree, not the user's checkout.
    // Git reports canonical paths (/private/var on macOS), so compare canonical forms.
    let child_worktree = PathBuf::from(first["worktree"].as_str().unwrap());
    let canonical_repo = outcome.repo.canonicalize().unwrap();
    assert!(child_worktree.starts_with(canonical_repo.join(".worktrees")));
    assert!(std::fs::read_to_string(child_worktree.join("README.md")).unwrap().contains("greeting"));
    assert!(!std::fs::read_to_string(outcome.repo.join("README.md")).unwrap().contains("greeting"));

    let (second_error, _, second_text) = tool_result(&outcome.log["results"][1]);
    assert!(second_error, "second attempt should be refused");
    assert!(second_text.contains("maxRetries"), "unexpected refusal: {second_text}");

    assert!(harness.sink.saw_status(&outcome.run_id, "senior-dev", "running"));
    assert!(harness.sink.saw_status(&outcome.run_id, "senior-dev", "idle"));
    let events = db.list_events(&outcome.run_id, 1000).unwrap();
    assert!(events.iter().any(|e| e.kind == "mcp_call"), "tool calls should be journaled");

    // Scenario 2: approval gate, approved by the operator.
    let approved = run_scenario(&harness, base, "approve", "DELEGATE=1 add a greeting", 3, true, Some(true)).await;
    let (error, parsed, text) = tool_result(&approved.log["results"][0]);
    assert!(!error, "approved delegation failed: {text}");
    assert_eq!(parsed["status"], "completed");

    // Scenario 3: approval gate, rejected by the operator. No child is ever provisioned.
    let rejected = run_scenario(&harness, base, "reject", "DELEGATE=1 add a greeting", 3, true, Some(false)).await;
    let (error, _, text) = tool_result(&rejected.log["results"][0]);
    assert!(error, "rejected delegation must surface as a tool error");
    assert!(text.contains("rejected"), "unexpected message: {text}");
    let worktrees = rejected.repo.join(".worktrees");
    let provisioned = worktrees.exists() && std::fs::read_dir(&worktrees).unwrap().count() > 0;
    assert!(!provisioned, "a rejected handoff must not create a worktree");

    // The gate was surfaced to the UI before the operator decided.
    assert!(harness.sink.saw_status(&approved.run_id, "senior-dev", "waiting_approval"));
}
