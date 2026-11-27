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

use condurence_lib::git::locks::RepoLocks;
use condurence_lib::graph::{default_graph, AccessLevel, OrchestrationGraph, RelationType, WorktreeMode};
use condurence_lib::mcp::{router::Router, server};
use condurence_lib::pty::PtyManager;
use condurence_lib::runtime::{events::EventSink, state_db::StateDb};
use serde::Serialize;
use serde_json::{json, Value};

const EDGE_ID: &str = "e-ceo-senior-dev";

/// Stand-in for `claude`. With `-p` it is a delegated child: it edits the worktree, then
/// calls `report_task_result`. Otherwise it is the root orchestrator: it delegates
/// `DELEGATE=<n>` times (default 1), records every tool result, and exits.
const FAKE_PROVIDER: &str = r#"#!/usr/bin/env python3
import json, re, subprocess, sys, time

argv = sys.argv[1:]
headless = "-p" in argv
config = json.load(open(argv[argv.index("--mcp-config") + 1]))
server = config["mcpServers"]["Condurence"]

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
    # A task that mentions SLOW never finishes on its own, so a test can abandon it mid-flight.
    if "SLOW" in argv[argv.index("-p") + 1]:
        time.sleep(120)
    if "NAP" in argv[argv.index("-p") + 1]:
        time.sleep(3)
    # WRITE=<text> makes the child append <text> instead, so parallel children can conflict.
    write = re.search(r"WRITE=(\S+)", argv[argv.index("-p") + 1])
    line = write.group(1) if write else "greeting: hello, world"
    with open("README.md", "a") as handle:
        handle.write("\n" + line + "\n")
    call("report_task_result", {"status": "completed", "summary": "Added greeting line to README.md"})
    print(json.dumps({"type": "result", "subtype": "success", "result": "Added greeting line to README.md"}))
    sys.exit(0)

prompt = argv[0]
match = re.search(r"DELEGATE=(\d+)", prompt)
count = int(match.group(1)) if match else 1
tools = rpc("tools/list")["result"]["tools"]
if "PLAN" in prompt:
    planned = call("plan_tasks", {"tasks": [
        {"id": "a", "title": "Add a greeting"},
        {"id": "b", "title": "Greet again", "depends_on": ["a"]}]})
    results = [planned]
    for task_id, text in [("b", "again"), ("a", "Add a greeting line to README.md"), ("b", "again")]:
        results.append(call("delegate_to_senior-dev", {"task": text, "acceptance_criteria": "README contains greeting", "task_id": task_id}))
    with open("orchestrator-log.json", "w") as handle:
        json.dump({"tools": [tool["name"] for tool in tools], "results": results}, handle)
    sys.exit(0)
parallel = re.search(r"PARALLEL=(\d+)", prompt)
if parallel:
    import threading
    waiting = {}
    def pump():
        for line in bridge.stdout:
            message = json.loads(line)
            slot = waiting.get(message.get("id"))
            if slot:
                slot["reply"] = message
                slot["done"].set()
    threading.Thread(target=pump, daemon=True).start()
    slots = []
    for _ in range(int(parallel.group(1))):
        counter[0] += 1
        slot = {"done": threading.Event(), "reply": None}
        waiting[counter[0]] = slot
        request = {"jsonrpc": "2.0", "id": counter[0], "method": "tools/call", "params": {
            "name": "delegate_to_senior-dev",
            "arguments": {"task": f"Add a greeting line to README.md NAP WRITE=line-{len(slots)}", "acceptance_criteria": "README contains greeting"}}}
        bridge.stdin.write(json.dumps(request) + "\n")
        bridge.stdin.flush()
        slots.append(slot)
    for slot in slots:
        slot["done"].wait(60)
    with open("orchestrator-log.json", "w") as handle:
        json.dump({"tools": [tool["name"] for tool in tools],
                   "results": [(slot["reply"] or {}).get("result") for slot in slots]}, handle)
    sys.exit(0)
results = []
task_id = re.search(r"TASK_ID=(\S+)", prompt)
for index in range(count):
    task = "Add a greeting line to README.md" if index == 0 else "again"
    if "SLOW" in prompt:
        task += " SLOW"
    arguments = {"task": task, "acceptance_criteria": "README contains greeting"}
    if task_id:
        arguments["task_id"] = task_id.group(1)
    results.append(call("delegate_to_senior-dev", arguments))
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

impl RecordingSink {
    /// The last PTY session id announced for `node_id` in `run_id`.
    fn session_of(&self, run_id: &str, node_id: &str) -> Option<String> {
        let wanted = format!("node-status://{node_id}");
        self.events
            .lock()
            .unwrap()
            .iter()
            .rev()
            .filter(|(name, payload)| *name == wanted && payload["runId"] == run_id)
            .find_map(|(_, payload)| payload["sessionId"].as_str().map(str::to_string))
    }

    /// The most recent status published for `node_id` in `run_id`.
    fn last_status(&self, run_id: &str, node_id: &str) -> Option<String> {
        let wanted = format!("node-status://{node_id}");
        self.events
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|(name, payload)| *name == wanted && payload["runId"] == run_id)
            .and_then(|(_, payload)| payload["status"].as_str().map(str::to_string))
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
    db: Arc<StateDb>,
    pty: Arc<PtyManager>,
}

/// `build_harness` points the process's `CONDURENCE_CLAUDE_BIN` at its own fake CLI, and a test
/// removes that file when it ends. Tests that build a harness therefore run one at a time, so no
/// test's child is started with another test's binary.
static HARNESS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn build_harness(db: Arc<StateDb>, provider: &Path) -> Harness {
    std::env::set_var("CONDURENCE_CLAUDE_BIN", provider);
    let sink = Arc::new(RecordingSink::default());
    let events: Arc<dyn EventSink> = sink.clone();
    let pty = Arc::new(PtyManager::new(events.clone()));
    let locks = Arc::new(RepoLocks::default());
    let (listener, port) = server::bind_loopback().unwrap();
    // The real binary serves --mcp-bridge, exactly as it does for an installed app.
    let bridge_exe = PathBuf::from(env!("CARGO_BIN_EXE_Condurence"));
    let router = Arc::new(Router::new(events, db.clone(), pty.clone(), locks, port, bridge_exe));
    server::serve(listener, router.clone()).unwrap();
    Harness { router, sink, db, pty }
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

#[allow(clippy::too_many_arguments)]
async fn run_scenario(
    harness: &Harness,
    base: &Path,
    name: &str,
    goal: &str,
    max_retries: u32,
    requires_approval: bool,
    approve: Option<bool>,
    gates: Option<&str>,
) -> ScenarioOutcome {
    let repo = make_repo(base, name);
    if let Some(text) = gates {
        std::fs::create_dir_all(repo.join(".orchestrator")).unwrap();
        std::fs::write(repo.join(".orchestrator").join("gates.json"), text).unwrap();
    }
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

    // The root exits once it has written its log, and that exit is what finishes the run. Wait
    // for the record before stopping, so the stop does not overwrite it.
    let db = harness.db.clone();
    let scoped = run_id.clone();
    let repo_key = repo.to_string_lossy().into_owned();
    let recorded_finished = wait_for(Duration::from_secs(10), move || {
        db.list_runs(&repo_key, 50)
            .unwrap()
            .iter()
            .any(|run| run.id == scoped && run.status == "finished")
    })
    .await;
    assert!(recorded_finished, "the run should be recorded as finished once its root exits ({name})");

    // The root's exit already took the run out of the active set. Stopping it is a harmless no-op
    // that keeps the recorded outcome and refuses the run's token.
    harness.router.stop_run(&run_id).await.expect("stop should succeed");
    let repo_key = repo.to_string_lossy().into_owned();
    let recorded = harness.db.list_runs(&repo_key, 50).unwrap();
    assert!(
        recorded.iter().any(|run| run.id == run_id && run.status == "finished"),
        "stopping a finished run must not overwrite its status ({name})"
    );
    ScenarioOutcome { run_id, log, repo }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn orchestration_end_to_end() {
    let _serial = HARNESS.lock().await;
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
    let outcome = run_scenario(&harness, base, "budget", "DELEGATE=2 add a greeting", 1, false, None, None).await;
    let tools = outcome.log["tools"].as_array().unwrap();
    assert!(tools.contains(&json!("delegate_to_senior-dev")), "{tools:?}");
    assert!(tools.contains(&json!("plan_tasks")), "the orchestrator plans: {tools:?}");
    assert!(!tools.contains(&json!("report_task_result")), "root must not see report_task_result");

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

    // The handoff is journaled with what was asked and what came back. The refused second attempt
    // never ran, so it has no handoff.
    let handoffs: Vec<_> = events.iter().filter(|e| e.kind == "handoff").collect();
    assert_eq!(handoffs.len(), 1, "only the attempt that ran is a handoff");
    let handoff = &handoffs[0].payload;
    assert_eq!(handoffs[0].node_id.as_deref(), Some("senior-dev"));
    assert_eq!(handoff["from"], "ceo");
    assert_eq!(handoff["to"], "senior-dev");
    assert_eq!(handoff["status"], "completed");
    assert!(handoff["prompt"].as_str().unwrap().contains("Add a greeting line to README.md"), "{handoff}");
    assert_eq!(handoff["summary"], "Added greeting line to README.md");
    assert!(handoff["diffStat"].as_str().unwrap().contains("README.md"));

    // Scenario 2: approval gate, approved by the operator.
    let approved = run_scenario(&harness, base, "approve", "DELEGATE=1 add a greeting", 3, true, Some(true), None).await;
    let (error, parsed, text) = tool_result(&approved.log["results"][0]);
    assert!(!error, "approved delegation failed: {text}");
    assert_eq!(parsed["status"], "completed");

    // Scenario 3: approval gate, rejected by the operator. No child is ever provisioned.
    let rejected = run_scenario(&harness, base, "reject", "DELEGATE=1 add a greeting", 3, true, Some(false), None).await;
    let (error, _, text) = tool_result(&rejected.log["results"][0]);
    assert!(error, "rejected delegation must surface as a tool error");
    assert!(text.contains("rejected"), "unexpected message: {text}");
    let worktrees = rejected.repo.join(".worktrees");
    let provisioned = worktrees.exists() && std::fs::read_dir(&worktrees).unwrap().count() > 0;
    assert!(!provisioned, "a rejected handoff must not create a worktree");

    // The gate was surfaced to the UI before the operator decided.
    assert!(harness.sink.saw_status(&approved.run_id, "senior-dev", "waiting_approval"));

    // Scenario 4: the root dies while a delegation is pending. Nobody is left to read the result,
    // so the child must not keep running (and spending tokens), and its card must not stay "running".
    let repo = make_repo(base, "abandon");
    let run_id = harness
        .router
        .start_run(repo.clone(), two_node_graph(3, false), "ceo".into(), "DELEGATE=1 SLOW add a greeting".into())
        .await
        .expect("run should start");
    let sink = harness.sink.clone();
    let scoped = run_id.clone();
    assert!(
        wait_for(Duration::from_secs(30), move || sink.saw_status(&scoped, "senior-dev", "running")).await,
        "the slow child should start"
    );
    let child_session = harness.sink.session_of(&run_id, "senior-dev").expect("the child announces its session");
    let root_session = harness.sink.session_of(&run_id, "ceo").expect("the root announces its session");
    assert!(harness.pty.get(&child_session).unwrap().is_running());

    // Both live agents have recorded who they are, which is what lets a launch after a crash
    // find and stop them. The start time must be the one the OS reports for that pid.
    let db_check = db.clone();
    let identified = wait_for(Duration::from_secs(5), move || {
        let live = db_check.unfinished_pty_sessions(i64::MAX).unwrap();
        live.len() >= 2 && live.iter().all(|row| row.pid.is_some() && row.started.is_some())
    })
    .await;
    assert!(identified, "running agents did not record their process identity");
    for row in db.unfinished_pty_sessions(i64::MAX).unwrap() {
        let current = condurence_lib::pty::process_group::process_identity(row.pid.unwrap());
        assert_eq!(current, row.started, "recorded identity must match the live process");
    }

    let pty = harness.pty.clone();
    tokio::task::spawn_blocking(move || pty.terminate(&root_session)).await.unwrap();

    let pty = harness.pty.clone();
    let child = child_session.clone();
    let stopped = wait_for(Duration::from_secs(20), move || !pty.get(&child).unwrap().is_running()).await;
    assert!(stopped, "the child kept running after its caller died");
    let sink = harness.sink.clone();
    let scoped = run_id.clone();
    let settled = wait_for(Duration::from_secs(10), move || sink.last_status(&scoped, "senior-dev").as_deref() != Some("running")).await;
    assert!(settled, "the abandoned child's card is stuck on running");
    let _ = harness.router.stop_run(&run_id).await;

    // Scenario 5: two delegations issued in one turn run side by side, each in its own worktree.
    // The bridge must not serialise them, and concurrent worktree creation must not collide.
    let parallel = run_scenario(&harness, base, "parallel", "PARALLEL=2 add a greeting", 3, false, None, None).await;
    let results = parallel.log["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    let mut worktrees = Vec::new();
    let mut landed = 0;
    for result in results {
        let (error, parsed, text) = tool_result(result);
        assert!(!error, "a parallel delegation failed: {text}");
        // Both children change the end of README.md from the same starting point. The first one
        // lands on the integration branch, and the second conflicts with it. The merge queue reports
        // that as a failed attempt rather than mixing the two edits silently.
        match parsed["merge"]["state"].as_str() {
            Some("merged") => {
                assert_eq!(parsed["status"], "completed", "{text}");
                landed += 1;
            }
            Some("conflict") => assert_eq!(parsed["status"], "failed", "{text}"),
            other => panic!("unexpected merge state {other:?}: {text}"),
        }
        worktrees.push(parsed["worktree"].as_str().unwrap().to_string());
    }
    assert_eq!(landed, 1, "one of two overlapping edits lands");
    assert_ne!(worktrees[0], worktrees[1], "parallel children must not share a worktree");

    // Every terminal session of the run, the root and both children, has its end recorded.
    let (db_check, run) = (db.clone(), parallel.run_id.clone());
    assert!(
        wait_for(Duration::from_secs(10), move || db_check.open_pty_sessions(&run).unwrap() == 0).await,
        "sessions were left without a recorded end"
    );

    // Both children were started before either finished, so they genuinely overlapped.
    let events = db.list_events(&parallel.run_id, 1000).unwrap();
    let senior: Vec<String> = events
        .iter()
        .filter(|e| e.kind == "status" && e.node_id.as_deref() == Some("senior-dev"))
        .map(|e| e.payload["status"].as_str().unwrap_or_default().to_string())
        .collect();
    let second_start = senior.iter().enumerate().filter(|(_, s)| *s == "running").nth(1).map(|(i, _)| i);
    // The card settles when the last child ends: "idle" when it landed, "error" when it conflicted.
    let first_finish = senior.iter().position(|s| s == "idle" || s == "error");
    assert!(
        matches!((second_start, first_finish), (Some(start), Some(finish)) if start < finish),
        "the children ran one after the other: {senior:?}"
    );
    // The card describes the node, so it settles once, when the last child ends, instead of
    // reading idle while one of them is still working.
    // (Stopping the run afterwards resets every card, so only the transitions before that count.)
    assert_eq!(senior[..2], ["running", "running"], "the card settled early: {senior:?}");
    assert!(matches!(senior[2].as_str(), "idle" | "error"), "the card never settled: {senior:?}");
}

/// Gates and per-task budgets through the real router. A failing gate overrides the child's
/// "completed" report. The same failure twice stalls a named task. A named task whose work passed
/// its gates cannot be retried under its id.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gates_and_task_budgets_end_to_end() {
    let _serial = HARNESS.lock().await;
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

    // The gate fails identically each time. The child says "completed" every time, and the
    // gate's exit code overrides it. The second identical failure stalls the named task, and the
    // third attempt is refused without starting an agent.
    let failing = r#"{"gates":[{"id":"check","command":"echo 'expected a farewell'; exit 1"}]}"#;
    let stalled = run_scenario(&harness, base, "stall", "DELEGATE=3 TASK_ID=login add a greeting", 5, false, None, Some(failing)).await;
    let results: Vec<(bool, Value, String)> = stalled.log["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(tool_result)
        .collect();
    assert_eq!(results.len(), 3, "{:?}", stalled.log);
    let (first_error, first, first_text) = &results[0];
    assert!(!first_error, "{first_text}");
    assert_eq!(first["status"], "failed", "a failing gate must override the report: {first_text}");
    assert_eq!(first["gates"][0]["id"], "check");
    assert_eq!(first["gates"][0]["passed"], false);
    assert_eq!(first["taskState"], "open");
    assert_eq!(first["task"], "login");
    assert_eq!(first["remaining"], 4);
    assert!(first["summary"].as_str().unwrap().contains("gate 'check' failed"), "{first_text}");
    let (second_error, second, second_text) = &results[1];
    assert!(!second_error, "{second_text}");
    assert_eq!(second["attempt"], 2);
    assert_eq!(second["taskState"], "stalled", "the same failure twice must stall the task");
    let (third_error, _, third_text) = &results[2];
    assert!(third_error, "a stalled task must be refused: {third_text}");
    assert!(third_text.contains("stalled"), "unexpected refusal: {third_text}");

    let events = db.list_events(&stalled.run_id, 1000).unwrap();
    let gate_events: Vec<_> = events.iter().filter(|e| e.kind == "gate").collect();
    assert_eq!(gate_events.len(), 2, "each attempt's gate run is journaled");
    assert!(gate_events.iter().all(|e| e.payload["passed"] == false));
    let handoffs: Vec<_> = events.iter().filter(|e| e.kind == "handoff").collect();
    assert_eq!(handoffs.len(), 2, "only the two attempts that ran are handoffs");
    assert_eq!(handoffs[1].payload["status"], "failed");

    // Gates that pass let the named task close. Retrying it under the same id is refused.
    let passing = r#"{"gates":[{"id":"check","command":"true"}]}"#;
    let accepted = run_scenario(&harness, base, "accept", "DELEGATE=2 TASK_ID=signup add a greeting", 5, false, None, Some(passing)).await;
    let results: Vec<(bool, Value, String)> = accepted.log["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(tool_result)
        .collect();
    assert_eq!(results.len(), 2, "{:?}", accepted.log);
    let (passed_error, passed, passed_text) = &results[0];
    assert!(!passed_error, "{passed_text}");
    assert_eq!(passed["status"], "completed");
    assert_eq!(passed["gates"][0]["passed"], true);
    assert_eq!(passed["taskState"], "accepted");
    let (refused_error, _, refused_text) = &results[1];
    assert!(refused_error, "a closed task must not be retried: {refused_text}");
    assert!(refused_text.contains("already accepted"), "unexpected refusal: {refused_text}");
}


/// A read-only delegate that changes its worktree fails, whatever it reports. The fake child
/// writes README.md and then says "completed", so the check is the only thing that catches it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn read_only_agents_cannot_change_their_worktree() {
    let _serial = HARNESS.lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let provider = tmp.path().join("claude");
    std::fs::write(&provider, FAKE_PROVIDER).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&provider, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let db = Arc::new(StateDb::open_in_memory().unwrap());
    let harness = build_harness(db.clone(), &provider);

    let repo = make_repo(tmp.path(), "readonly");
    let mut graph = two_node_graph(3, false);
    for node in graph.nodes.iter_mut().filter(|node| node.id == "senior-dev") {
        node.data.access = AccessLevel::ReadOnly;
    }
    let run_id = harness
        .router
        .start_run(repo.clone(), graph, "ceo".into(), "DELEGATE=1 add a greeting".into())
        .await
        .expect("run should start");

    let log_path = repo.join("orchestrator-log.json");
    assert!(wait_for(Duration::from_secs(60), || log_path.exists()).await, "root never finished");
    let log: Value = serde_json::from_str(&std::fs::read_to_string(&log_path).unwrap()).unwrap();
    let (error, parsed, text) = tool_result(&log["results"][0]);
    assert!(!error, "{text}");
    assert_eq!(parsed["status"], "failed", "a read-only change must fail the attempt: {text}");
    assert!(text.contains("read-only"), "the caller must be told why: {text}");

    let events = db.list_events(&run_id, 1000).unwrap();
    let handoff = events.iter().find(|e| e.kind == "handoff").expect("the attempt is journaled");
    assert_eq!(handoff.payload["status"], "failed");
}

/// A planned run: the orchestrator plans two tasks where `b` depends on `a`. It tries `b` first,
/// which the router refuses, then `a`, then `b` again, which now runs. The journal records the plan
/// and every transition, so the run can be read back task by task.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn planned_tasks_wait_for_their_dependencies() {
    let _serial = HARNESS.lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let provider = tmp.path().join("claude");
    std::fs::write(&provider, FAKE_PROVIDER).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&provider, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let db = Arc::new(StateDb::open_in_memory().unwrap());
    let harness = build_harness(db.clone(), &provider);

    let repo = make_repo(tmp.path(), "planned");
    let run_id = harness
        .router
        .start_run(repo.clone(), two_node_graph(3, false), "ceo".into(), "PLAN add a greeting".into())
        .await
        .expect("run should start");

    let log_path = repo.join("orchestrator-log.json");
    assert!(wait_for(Duration::from_secs(60), || log_path.exists()).await, "root never finished");
    let log: Value = serde_json::from_str(&std::fs::read_to_string(&log_path).unwrap()).unwrap();
    let results: Vec<(bool, Value, String)> = log["results"].as_array().unwrap().iter().map(tool_result).collect();
    assert_eq!(results.len(), 4, "{:?}", log);

    let (planned_error, _, planned_text) = &results[0];
    assert!(!planned_error, "{planned_text}");

    let (early_error, _, early_text) = &results[1];
    assert!(early_error, "b must wait for a: {early_text}");
    assert!(early_text.contains("waits for a"), "unexpected refusal: {early_text}");

    let (first_error, first, first_text) = &results[2];
    assert!(!first_error, "{first_text}");
    assert_eq!(first["task"], "a");
    assert_eq!(first["taskState"], "accepted");

    let (later_error, later, later_text) = &results[3];
    assert!(!later_error, "b may run once a is accepted: {later_text}");
    assert_eq!(later["status"], "completed");

    let events = db.list_events(&run_id, 1000).unwrap();
    let plan = events.iter().find(|e| e.kind == "plan").expect("the plan is journaled");
    assert_eq!(plan.payload["tasks"].as_array().unwrap().len(), 2);
    let refused: Vec<_> = events
        .iter()
        .filter(|e| e.kind == "task" && e.payload["state"] == "refused")
        .collect();
    assert_eq!(refused.len(), 1, "exactly the early attempt is refused");
    assert_eq!(refused[0].payload["taskId"], "b");
    assert!(refused[0].payload["reason"].as_str().unwrap().contains("waits for a"));
    let accepted_a = events
        .iter()
        .any(|e| e.kind == "task" && e.payload["taskId"] == "a" && e.payload["state"] == "accepted");
    assert!(accepted_a, "a's acceptance is journaled as a task event");
}

/// Runs `goal` through `graph` from `repo` and returns the run id and the tool results the root got.
async fn delegate_through(
    harness: &Harness,
    repo: &Path,
    graph: OrchestrationGraph,
    goal: &str,
) -> (String, Vec<(bool, Value, String)>) {
    let run_id = harness
        .router
        .start_run(repo.to_path_buf(), graph, "ceo".into(), goal.into())
        .await
        .expect("run should start");
    let log_path = repo.join("orchestrator-log.json");
    assert!(wait_for(Duration::from_secs(60), || log_path.exists()).await, "root never finished");
    let log: Value = serde_json::from_str(&std::fs::read_to_string(&log_path).unwrap()).unwrap();
    let results = log["results"].as_array().unwrap().iter().map(tool_result).collect();
    (run_id, results)
}

fn git_show(repo: &Path, spec: &str) -> String {
    let output = Command::new("git").current_dir(repo).args(["show", spec]).output().unwrap();
    assert!(output.status.success(), "git show {spec} failed");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Two isolated agents run one after the other. Each one branches from the integration tip, so the
/// second one starts from the first one's accepted work, and both land on the integration branch.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn isolated_work_lands_on_the_integration_branch_in_turn() {
    let _serial = HARNESS.lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let provider = tmp.path().join("claude");
    std::fs::write(&provider, FAKE_PROVIDER).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&provider, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let db = Arc::new(StateDb::open_in_memory().unwrap());
    let harness = build_harness(db.clone(), &provider);

    let repo = make_repo(tmp.path(), "merge-sequence");
    let (run_id, results) = delegate_through(&harness, &repo, two_node_graph(3, false), "DELEGATE=2 add a greeting").await;
    assert_eq!(results.len(), 2, "{results:?}");
    for (error, parsed, text) in &results {
        assert!(!error, "{text}");
        assert_eq!(parsed["status"], "completed", "{text}");
        assert_eq!(parsed["merge"]["state"], "merged", "{text}");
    }
    let into = results[1].1["merge"]["into"].as_str().unwrap().to_string();
    assert_eq!(into, format!("oc/{}/integration", &run_id[..8]));
    let landed = git_show(&repo, &format!("{into}:README.md"));
    assert_eq!(landed.matches("greeting: hello, world").count(), 2, "both pieces of work are on the branch: {landed}");

    let events = db.list_events(&run_id, 1000).unwrap();
    let merges: Vec<_> = events.iter().filter(|e| e.kind == "merge").collect();
    assert_eq!(merges.len(), 2, "each isolated attempt is journaled as a merge");
    assert!(merges.iter().all(|e| e.payload["state"] == "merged"));
}

/// Two isolated agents start from the same tip and both change the end of the same file. The first
/// lands. The second conflicts, so its attempt fails with the conflict named, and nothing is left
/// half-merged on the integration branch.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn conflicting_isolated_work_fails_cleanly() {
    let _serial = HARNESS.lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let provider = tmp.path().join("claude");
    std::fs::write(&provider, FAKE_PROVIDER).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&provider, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let db = Arc::new(StateDb::open_in_memory().unwrap());
    let harness = build_harness(db.clone(), &provider);

    let repo = make_repo(tmp.path(), "merge-conflict");
    let (run_id, results) = delegate_through(&harness, &repo, two_node_graph(3, false), "PARALLEL=2 add a greeting").await;
    assert_eq!(results.len(), 2, "{results:?}");
    let merged: Vec<_> = results.iter().filter(|(_, p, _)| p["merge"]["state"] == "merged").collect();
    let conflicted: Vec<_> = results.iter().filter(|(_, p, _)| p["merge"]["state"] == "conflict").collect();
    assert_eq!(merged.len(), 1, "exactly one of them lands: {results:?}");
    assert_eq!(conflicted.len(), 1, "the other one conflicts: {results:?}");
    let (_, failed, failed_text) = conflicted[0];
    assert_eq!(failed["status"], "failed", "{failed_text}");
    assert!(
        failed["summary"].as_str().unwrap().contains("conflicts with the integration branch"),
        "{failed_text}"
    );

    let events = db.list_events(&run_id, 1000).unwrap();
    assert!(events.iter().any(|e| e.kind == "merge" && e.payload["state"] == "conflict"));
}

/// A shared writer works in its caller's worktree, so a second writer there is refused while the
/// first one runs. The refusal names the busy worktree and does not charge the task's budget.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_writer_in_a_shared_worktree_is_refused() {
    let _serial = HARNESS.lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let provider = tmp.path().join("claude");
    std::fs::write(&provider, FAKE_PROVIDER).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&provider, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let db = Arc::new(StateDb::open_in_memory().unwrap());
    let harness = build_harness(db.clone(), &provider);

    let repo = make_repo(tmp.path(), "shared-busy");
    let mut graph = two_node_graph(3, false);
    for node in graph.nodes.iter_mut().filter(|node| node.id == "senior-dev") {
        node.data.worktree_mode = WorktreeMode::Shared;
    }
    let (_, results) = delegate_through(&harness, &repo, graph, "PARALLEL=2 add a greeting").await;
    assert_eq!(results.len(), 2, "{results:?}");
    let refused: Vec<_> = results.iter().filter(|(error, _, _)| *error).collect();
    let ran: Vec<_> = results.iter().filter(|(error, _, _)| !*error).collect();
    assert_eq!(refused.len(), 1, "exactly one writer is refused: {results:?}");
    assert_eq!(ran.len(), 1, "exactly one writer runs: {results:?}");
    assert!(refused[0].2.contains("busy"), "{}", refused[0].2);
    assert_eq!(ran[0].1["status"], "completed");
}

/// One scripted scenario of the eval suite. The fake CLI does what the goal's markers say, so every
/// scenario has a known right answer, and the suite checks the runtime against it.
struct EvalCase {
    name: &'static str,
    goal: &'static str,
    graph: fn() -> OrchestrationGraph,
    gates: Option<&'static str>,
}

const EVAL_PASSING_GATE: &str = r#"{"gates":[{"id":"check","command":"true"}]}"#;
const EVAL_FAILING_GATE: &str = r#"{"gates":[{"id":"check","command":"echo 'expected a farewell'; exit 1"}]}"#;

fn shared_writer_graph() -> OrchestrationGraph {
    let mut graph = two_node_graph(3, false);
    for node in graph.nodes.iter_mut().filter(|node| node.id == "senior-dev") {
        node.data.worktree_mode = WorktreeMode::Shared;
    }
    graph
}

fn read_only_reviewer_graph() -> OrchestrationGraph {
    let mut graph = two_node_graph(3, false);
    for node in graph.nodes.iter_mut().filter(|node| node.id == "senior-dev") {
        node.data.access = AccessLevel::ReadOnly;
    }
    graph
}

fn eval_cases() -> Vec<EvalCase> {
    vec![
        EvalCase {
            name: "gated-completion",
            goal: "DELEGATE=1 add a greeting",
            graph: || two_node_graph(3, false),
            gates: Some(EVAL_PASSING_GATE),
        },
        EvalCase {
            name: "failing-gate-stalls",
            goal: "DELEGATE=3 TASK_ID=login add a greeting",
            graph: || two_node_graph(5, false),
            gates: Some(EVAL_FAILING_GATE),
        },
        EvalCase {
            name: "dependency-order",
            goal: "PLAN add a greeting",
            graph: || two_node_graph(3, false),
            gates: None,
        },
        EvalCase {
            name: "isolated-overlap",
            goal: "PARALLEL=2 add a greeting",
            graph: || two_node_graph(3, false),
            gates: None,
        },
        EvalCase {
            name: "shared-writer",
            goal: "PARALLEL=2 add a greeting",
            graph: shared_writer_graph,
            gates: None,
        },
        EvalCase {
            name: "read-only-reviewer",
            goal: "DELEGATE=1 add a greeting",
            graph: read_only_reviewer_graph,
            gates: None,
        },
    ]
}

/// What one scenario did, counted from the tool results the orchestrator received.
#[derive(Debug, Serialize)]
struct EvalMetrics {
    case: &'static str,
    calls: usize,
    /// Calls the router refused before an agent ran: a missing dependency, a busy worktree, an
    /// exhausted budget, a stalled or already accepted task.
    refused: usize,
    /// Agent attempts that ran and reported `completed` after their gates.
    completed: usize,
    failed: usize,
    accepted: usize,
    stalled: usize,
    gate_failures: usize,
    merged: usize,
    conflicts: usize,
    duration_ms: u128,
}

fn eval_metrics(case: &'static str, results: &[(bool, Value, String)], elapsed: Duration) -> EvalMetrics {
    let count = |predicate: &dyn Fn(&(bool, Value, String)) -> bool| results.iter().filter(|r| predicate(r)).count();
    EvalMetrics {
        case,
        calls: results.len(),
        refused: count(&|r| r.0),
        completed: count(&|r| !r.0 && r.1["status"] == "completed"),
        failed: count(&|r| !r.0 && r.1["status"] == "failed"),
        accepted: count(&|r| !r.0 && r.1["taskState"] == "accepted"),
        stalled: count(&|r| !r.0 && r.1["taskState"] == "stalled"),
        gate_failures: results
            .iter()
            .map(|r| r.1["gates"].as_array().map(|gates| gates.iter().filter(|g| g["passed"] == false).count()).unwrap_or(0))
            .sum(),
        merged: count(&|r| !r.0 && r.1["merge"]["state"] == "merged"),
        conflicts: count(&|r| !r.0 && r.1["merge"]["state"] == "conflict"),
        duration_ms: elapsed.as_millis(),
    }
}

/// Each scenario must do what its design says. Returns a reason when it does not.
fn eval_expectation(case: &str, results: &[(bool, Value, String)], metrics: &EvalMetrics) -> Result<(), String> {
    let check = |ok: bool, what: &str| if ok { Ok(()) } else { Err(format!("{case}: {what}")) };
    match case {
        "gated-completion" => {
            check(metrics.calls == 1 && metrics.completed == 1 && metrics.refused == 0, "one clean completion")?;
            check(results[0].1["gates"][0]["passed"] == true, "the gate passed")
        }
        "failing-gate-stalls" => {
            check(metrics.calls == 3, "three calls")?;
            check(metrics.gate_failures == 2, "each of the first two attempts failed its gate")?;
            check(metrics.stalled == 1 && metrics.refused == 1, "the repeat stalls the task and the third call is refused")
        }
        "dependency-order" => {
            check(metrics.refused == 1, "the call that jumps the plan is refused")?;
            check(metrics.accepted == 2, "both planned tasks end accepted, in dependency order")
        }
        "isolated-overlap" => {
            check(metrics.merged == 1 && metrics.conflicts == 1, "one overlapping edit lands and the other conflicts")
        }
        "shared-writer" => check(metrics.refused == 1 && metrics.completed == 1, "only one writer runs in the shared worktree"),
        "read-only-reviewer" => check(metrics.failed == 1 && metrics.completed == 0, "the read-only change fails the attempt"),
        other => Err(format!("unknown case {other}")),
    }
}

/// Runs every scripted scenario through the real router, checks each one, prints a metrics table and
/// writes the same numbers to a JSON report, so a change to the runtime or to a topology can be
/// judged by the numbers rather than by how the log reads.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn eval_suite_reports_the_runtime_behaviour() {
    let _serial = HARNESS.lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let provider = tmp.path().join("claude");
    std::fs::write(&provider, FAKE_PROVIDER).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&provider, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let db = Arc::new(StateDb::open_in_memory().unwrap());
    let harness = build_harness(db.clone(), &provider);

    let mut report = Vec::new();
    let mut failures = Vec::new();
    for case in eval_cases() {
        let repo = make_repo(tmp.path(), case.name);
        if let Some(gates) = case.gates {
            std::fs::create_dir_all(repo.join(".orchestrator")).unwrap();
            std::fs::write(repo.join(".orchestrator").join("gates.json"), gates).unwrap();
        }
        let started = Instant::now();
        let (_, results) = delegate_through(&harness, &repo, (case.graph)(), case.goal).await;
        let metrics = eval_metrics(case.name, &results, started.elapsed());
        if let Err(reason) = eval_expectation(case.name, &results, &metrics) {
            failures.push(reason);
        }
        report.push(metrics);
    }

    println!("\n{:<20} {:>5} {:>7} {:>9} {:>6} {:>8} {:>7} {:>6} {:>6} {:>9} {:>8}", "case", "calls", "refused", "completed", "failed", "accepted", "stalled", "gates!", "merged", "conflicts", "ms");
    for m in &report {
        println!(
            "{:<20} {:>5} {:>7} {:>9} {:>6} {:>8} {:>7} {:>6} {:>6} {:>9} {:>8}",
            m.case, m.calls, m.refused, m.completed, m.failed, m.accepted, m.stalled, m.gate_failures, m.merged, m.conflicts, m.duration_ms
        );
    }
    let dir = std::env::temp_dir().join("condurence-evals");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("report.json");
    std::fs::write(&path, serde_json::to_string_pretty(&report).unwrap()).unwrap();
    println!("report written to {}", path.display());
    assert!(failures.is_empty(), "eval expectations failed:\n{}", failures.join("\n"));
}

/// A small repository with one seeded bug: `add` subtracts. The gate is the check that must pass.
fn seeded_bug_repo(parent: &Path) -> PathBuf {
    let repo = make_repo(parent, "seeded-bug");
    std::fs::write(repo.join("add.js"), "module.exports = (a, b) => a - b;\n").unwrap();
    std::fs::write(
        repo.join("test.js"),
        "const add = require('./add');\nif (add(2, 3) !== 5) {\n  console.error('add(2, 3) should be 5');\n  process.exit(1);\n}\nconsole.log('ok');\n",
    )
    .unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "seed the bug"]);
    std::fs::create_dir_all(repo.join(".orchestrator")).unwrap();
    std::fs::write(repo.join(".orchestrator").join("gates.json"), r#"{"gates":[{"id":"test","command":"node test.js"}]}"#).unwrap();
    repo
}

/// Live evaluation: the same runtime with the real provider CLIs, on the seeded bug. It spends the
/// user's provider subscription, so it does not run by default:
///
///   cargo test --test orchestration_e2e live_eval -- --ignored --nocapture
///
/// The orchestrator is interactive, so a live run is watched through its journal and stopped once
/// the task has an answer, or after `LIVE_EVAL_LIMIT`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "spends real provider tokens"]
async fn live_eval_fixes_a_seeded_bug() {
    const LIVE_EVAL_LIMIT: Duration = Duration::from_secs(15 * 60);
    let _serial = HARNESS.lock().await;
    let claude = which::which("claude").expect("the claude CLI must be on PATH for a live eval");
    let tmp = tempfile::tempdir().unwrap();
    let db = Arc::new(StateDb::open_in_memory().unwrap());
    let harness = build_harness(db.clone(), &claude);

    let repo = seeded_bug_repo(tmp.path());
    let mut graph = two_node_graph(3, false);
    for node in graph.nodes.iter_mut() {
        node.data.model = "claude-haiku-5-5".into();
    }
    let started = Instant::now();
    let run_id = harness
        .router
        .start_run(
            repo.clone(),
            graph,
            "ceo".into(),
            "The check in test.js fails because add() in add.js subtracts. Fix add() so that `node test.js` prints ok. Delegate the fix to the Senior Developer with task_id fix-add, and stop once it is accepted.".into(),
        )
        .await
        .expect("run should start");

    let answered = |events: &[condurence_lib::runtime::state_db::RunEvent]| {
        events.iter().any(|e| e.kind == "task" && matches!(e.payload["state"].as_str(), Some("accepted") | Some("stalled")))
    };
    let deadline = Instant::now() + LIVE_EVAL_LIMIT;
    loop {
        let events = db.list_events(&run_id, 2000).unwrap();
        if answered(&events) || Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
    harness.router.stop_run(&run_id).await.expect("stop should succeed");
    let events = db.list_events(&run_id, 2000).unwrap();

    let state_of = |kind: &str, state: &str| events.iter().filter(|e| e.kind == kind && e.payload["state"] == state).count();
    let metrics = EvalMetrics {
        case: "live-seeded-bug",
        calls: events.iter().filter(|e| e.kind == "task" && matches!(e.payload["state"].as_str(), Some("running") | Some("refused"))).count(),
        refused: state_of("task", "refused"),
        completed: events.iter().filter(|e| e.kind == "handoff" && e.payload["status"] == "completed").count(),
        failed: events.iter().filter(|e| e.kind == "handoff" && e.payload["status"] == "failed").count(),
        accepted: state_of("task", "accepted"),
        stalled: state_of("task", "stalled"),
        gate_failures: events.iter().filter(|e| e.kind == "gate" && e.payload["passed"] == false).count(),
        merged: state_of("merge", "merged"),
        conflicts: state_of("merge", "conflict"),
        duration_ms: started.elapsed().as_millis(),
    };
    println!("{}", serde_json::to_string_pretty(&metrics).unwrap());
    let dir = std::env::temp_dir().join("condurence-evals");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("live-report.json"), serde_json::to_string_pretty(&metrics).unwrap()).unwrap();
    assert!(metrics.accepted >= 1, "the fix was never accepted: {metrics:?}");
}
