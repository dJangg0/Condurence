//! LIVE end-to-end orchestration against the real provider CLIs. Nothing is faked: a real
//! PTY, the real loopback MCP server, the real `--mcp-bridge` shim, the real router and the
//! real `claude` / `codex` binaries signed in with the developer's own accounts.
//!
//! It spends real subscription usage, so every test here is `#[ignore]`. Run on purpose with:
//!
//! ```text
//! cargo test --test live_orchestration -- --ignored --nocapture --test-threads=1
//! ```
//!
//! Every Claude node uses Haiku 5.5 at the lowest effort to keep the cost to a few cents.
//! The tests print the root's terminal, the event journal and each child's terminal, so a
//! failure shows where the setup broke.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use condurence_lib::git::locks::RepoLocks;
use condurence_lib::graph::{
    default_graph, AgentProvider, GraphEdge, GraphNode, OrchestrationGraph, RelationType, RoutingEdgeData, WorktreeMode,
};
use condurence_lib::mcp::router::strip_ansi;
use condurence_lib::mcp::{router::Router, server};
use condurence_lib::pty::PtyManager;
use condurence_lib::runtime::{events::EventSink, state_db::StateDb};
use serde_json::Value;

const HAIKU: &str = "claude-haiku-5-5";
const EFFORT: &str = "low";
const RUN_LIMIT: Duration = Duration::from_secs(240);

#[derive(Default)]
struct RecordingSink {
    events: Mutex<Vec<(String, Value)>>,
}

impl RecordingSink {
    /// The most recent PTY session id reported for a node, if any.
    fn session_of(&self, node_id: &str) -> Option<String> {
        let wanted = format!("node-status://{node_id}");
        self.events
            .lock()
            .unwrap()
            .iter()
            .rev()
            .filter(|(name, _)| *name == wanted)
            .find_map(|(_, payload)| payload["sessionId"].as_str().map(str::to_string))
    }

    fn statuses_of(&self, node_id: &str) -> Vec<String> {
        let wanted = format!("node-status://{node_id}");
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|(name, _)| *name == wanted)
            .filter_map(|(_, payload)| payload["status"].as_str().map(str::to_string))
            .collect()
    }

    fn last_summary_of(&self, node_id: &str) -> Option<String> {
        let wanted = format!("node-status://{node_id}");
        self.events
            .lock()
            .unwrap()
            .iter()
            .rev()
            .filter(|(name, _)| *name == wanted)
            .find_map(|(_, payload)| payload["summary"].as_str().map(str::to_string))
    }
}

impl EventSink for RecordingSink {
    fn emit(&self, event: &str, payload: Value) {
        self.events.lock().unwrap().push((event.to_string(), payload));
    }
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git").current_dir(dir).args(args).status().unwrap();
    assert!(status.success(), "git {args:?} failed");
}

fn make_repo(parent: &Path) -> PathBuf {
    let repo = parent.join("live-demo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "Condurence Live Test"]);
    std::fs::write(repo.join("README.md"), "# live demo\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    repo
}

fn node(id: &str, name: &str, provider: AgentProvider, model: &str, effort: &str, prompt: &str, mode: WorktreeMode) -> GraphNode {
    let mut template = default_graph().nodes.remove(0);
    template.id = id.into();
    template.data.name = name.into();
    template.data.role = name.into();
    template.data.provider = provider;
    template.data.model = model.into();
    template.data.effort = effort.into();
    template.data.skills = vec![];
    template.data.context_paths = vec![];
    template.data.system_prompt = prompt.into();
    template.data.worktree_mode = mode;
    template
}

/// `ceo` (Claude Haiku, low effort) delegates to `greeter` on the given provider.
fn graph(child_provider: AgentProvider) -> OrchestrationGraph {
    let child_model = if child_provider == AgentProvider::Claude { HAIKU } else { "" };
    let child_effort = if child_provider == AgentProvider::Opencode { "" } else { EFFORT };
    let mut graph = default_graph();
    graph.nodes = vec![
        node(
            "ceo",
            "Orchestrator",
            AgentProvider::Claude,
            HAIKU,
            EFFORT,
            "You coordinate other agents. Use your delegate tool for the work, then summarise the reply in one line.",
            WorktreeMode::Shared,
        ),
        node(
            "greeter",
            "Greeter",
            child_provider,
            child_model,
            child_effort,
            "You are a friendly greeter. Reply with one short hello sentence. Do not edit files.",
            WorktreeMode::Isolated,
        ),
    ];
    graph.edges = vec![GraphEdge {
        id: "e-ceo-greeter".into(),
        source: "ceo".into(),
        target: "greeter".into(),
        data: RoutingEdgeData { relation_type: RelationType::DelegatesTo, max_retries: 1, requires_approval: false },
    }];
    condurence_lib::graph::validate(&graph).expect("live graph must validate");
    graph
}

struct Report {
    root_screen: String,
    child_screen: String,
    journal: Vec<(String, Option<String>, Value)>,
    greeter_statuses: Vec<String>,
    greeter_summary: Option<String>,
    root_answers: u32,
    timed_out: bool,
}

/// What the operator would press for each prompt Claude's terminal UI can show. Matching is done
/// on text with all whitespace removed, because the TUI positions words with cursor moves and the
/// ANSI-stripped stream loses the spaces between them.
///
/// The trust screen selects "No, exit" by default, so it needs Down then Enter. Permission
/// prompts select the affirmative option by default, so Enter alone is enough.
const PROMPTS: [(&str, &[u8]); 4] = [
    ("yes,itrustthisfolder", b"\x1b[B\r"),
    ("doyouwanttoproceed", b"\r"),
    ("doyouwanttoallow", b"\r"),
    ("1.yes", b"\r"),
];

fn squash(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase).collect()
}

async fn run_live(child_provider: AgentProvider) -> Report {
    let tmp = tempfile::tempdir().unwrap();
    let repo = make_repo(tmp.path());
    let db = Arc::new(StateDb::open_in_memory().unwrap());
    let sink = Arc::new(RecordingSink::default());
    let events: Arc<dyn EventSink> = sink.clone();
    let pty = Arc::new(PtyManager::new(events.clone()));
    let locks = Arc::new(RepoLocks::default());
    let (listener, port) = server::bind_loopback().unwrap();
    let bridge_exe = PathBuf::from(env!("CARGO_BIN_EXE_Condurence"));
    let router = Arc::new(Router::new(events, db.clone(), pty.clone(), locks, port, bridge_exe));
    server::serve(listener, router.clone()).unwrap();

    let goal = "Ask the greeter agent to say hello, then tell me exactly what it replied.";
    let run_id = router
        .start_run(repo.clone(), graph(child_provider), "ceo".into(), goal.into())
        .await
        .expect("run should start");

    let started = Instant::now();
    let mut root_answers = 0u32;
    let mut last_answer = Instant::now() - Duration::from_secs(60);
    // Only output that arrived after the previous answer can contain a new prompt.
    let mut answered_upto = 0usize;
    let mut timed_out = false;
    loop {
        // Answer the root's trust and permission prompts, as the operator would in the pane.
        if let Some(session) = sink.session_of("ceo").and_then(|id| pty.get(&id).ok()) {
            let screen = squash(&strip_ansi(&session.history()));
            let fresh = if screen.len() >= answered_upto { &screen[answered_upto..] } else { &screen[..] };
            let hit = PROMPTS.iter().find(|(marker, _)| fresh.contains(marker));
            if let Some((marker, keys)) = hit {
                if root_answers < 8 && last_answer.elapsed() > Duration::from_secs(3) {
                    let _ = session.write(keys);
                    root_answers += 1;
                    last_answer = Instant::now();
                    answered_upto = screen.len();
                    println!("[live] root prompt #{root_answers}: answered '{marker}'");
                }
            }
        }
        let statuses = sink.statuses_of("greeter");
        let child_done = statuses.last().is_some_and(|s| s == "idle" || s == "error") && statuses.iter().any(|s| s == "running");
        if child_done && started.elapsed() > Duration::from_secs(5) {
            // Let the root see the tool result before the run is stopped.
            tokio::time::sleep(Duration::from_secs(8)).await;
            break;
        }
        if started.elapsed() > RUN_LIMIT {
            timed_out = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    let screen_of = |node: &str| {
        sink.session_of(node)
            .and_then(|id| pty.get(&id).ok())
            .map(|s| strip_ansi(&s.history()))
            .unwrap_or_default()
    };
    let report = Report {
        root_screen: screen_of("ceo"),
        child_screen: screen_of("greeter"),
        journal: db
            .list_events(&run_id, 1000)
            .unwrap()
            .into_iter()
            .map(|e| (e.kind, e.node_id, e.payload))
            .collect(),
        greeter_statuses: sink.statuses_of("greeter"),
        greeter_summary: sink.last_summary_of("greeter"),
        root_answers,
        timed_out,
    };
    let _ = router.stop_run(&run_id).await;
    report
}

fn tail(text: &str, max: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    chars[chars.len().saturating_sub(max)..].iter().collect()
}

fn print_report(title: &str, report: &Report) {
    println!("\n================ {title} ================");
    println!("timed out: {} | root prompts answered: {}", report.timed_out, report.root_answers);
    println!("greeter statuses: {:?}", report.greeter_statuses);
    println!("greeter summary: {:?}", report.greeter_summary);
    println!("--- journal ({} events) ---", report.journal.len());
    for (kind, node, payload) in &report.journal {
        let text = payload.to_string();
        println!("  {kind:<10} {:<8} {}", node.clone().unwrap_or_default(), tail(&text, 400));
    }
    println!("--- root terminal (tail) ---\n{}", tail(&report.root_screen, 2500));
    println!("--- greeter terminal (tail) ---\n{}", tail(&report.child_screen, 1500));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "spends real Claude usage; run with --ignored"]
async fn live_claude_orchestrator_delegates_hello_to_claude_greeter() {
    let report = run_live(AgentProvider::Claude).await;
    print_report("claude -> claude (haiku 5.5, low effort)", &report);

    assert!(!report.timed_out, "the run timed out: see the printed terminals above");
    assert!(report.greeter_statuses.iter().any(|s| s == "running"), "the greeter never started");
    assert_eq!(report.greeter_statuses.last().map(String::as_str), Some("idle"), "the greeter did not finish cleanly");
    let called = report.journal.iter().any(|(kind, _, _)| kind == "mcp_call");
    assert!(called, "no inter-agent tool call was journaled");
    // The child must reach the explicit completion tool, not just exit. A refusal here means the
    // MCP permission setup is broken for headless Claude.
    let reported = report
        .journal
        .iter()
        .any(|(kind, _, payload)| kind == "mcp_call" && payload["tool"] == "report_task_result");
    assert!(reported, "the greeter never called report_task_result: {:?}", report.greeter_summary);
    let complained = report.greeter_summary.as_deref().is_some_and(|t| t.to_lowercase().contains("permission"));
    assert!(!complained, "the greeter reported a permission problem: {:?}", report.greeter_summary);
    let said_hello = report
        .greeter_summary
        .as_deref()
        .is_some_and(|text| text.to_lowercase().contains("hello") || text.to_lowercase().contains("hi"));
    assert!(said_hello, "the greeter's reply did not come back: {:?}", report.greeter_summary);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "spends real Claude and Codex usage; run with --ignored"]
async fn live_codex_child_reply_comes_back_as_its_summary() {
    let report = run_live(AgentProvider::Codex).await;
    print_report("claude -> codex", &report);

    assert!(!report.timed_out, "the run timed out: see the printed terminals above");
    assert!(report.greeter_statuses.iter().any(|s| s == "running"), "the codex greeter never started");
    assert_eq!(report.greeter_statuses.last().map(String::as_str), Some("idle"), "a child that answered must end idle");
    // Codex may not call `report_task_result`, so the card comes from its final reply. That is the
    // greeting itself, not the token counter Codex prints after it or the prompt it echoed first.
    let card = report.greeter_summary.clone().unwrap_or_default().to_lowercase();
    assert!(card.contains("hello") || card.contains("hi"), "the card is not the greeting: {card:?}");
    assert!(!card.contains("## completion"), "the card shows the echoed prompt: {card:?}");
    assert!(!card.chars().all(|c| c.is_ascii_digit() || c == ','), "the card is the token counter: {card:?}");
}
