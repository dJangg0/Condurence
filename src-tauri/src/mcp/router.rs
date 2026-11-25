//! Deterministic handoff state machine (HANDOFF §4.4.4).
//!
//! The root orchestrator runs interactively in a PTY. When it calls a `delegate_to_*`,
//! `verify_with_*` or `escalate_to_*` tool, the router:
//!   1. validates the edge exists and that the hop does not form a forbidden cycle,
//!   2. enforces the edge's `maxRetries` attempt budget,
//!   3. pauses on an approval gate when `requiresApproval` is set,
//!   4. provisions the target's worktree (shared with the caller, or an isolated child),
//!   5. spawns the target headless in a visible PTY with an injected `report_task_result`,
//!   6. blocks the caller's `tools/call` until the child reports or exits, and returns the
//!      summary, exit code and `git diff --stat`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use dashmap::DashMap;
use serde_json::{json, Value};
use tokio::sync::{oneshot, watch};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::compiler::{self, BridgeAddress, Invocation, Mode, REPORT_TOOL};
use crate::env::resolver;
use crate::error::{AppError, AppResult};
use crate::git::{cmd, diff_engine::EMPTY_TREE, locks::RepoLocks, worktree};
use crate::graph::{GraphNode, NodeStatus, OrchestrationGraph, RelationType, WorktreeMode};
use crate::pty::{PtyManager, PtySession, SpawnSpec};
use crate::runtime::{events::EventSink, now_ms, state_db::StateDb};

const PROTOCOL_VERSION: &str = "2025-06-18";
/// After a child reports, how long it may keep running before we terminate it.
const REPORT_GRACE: Duration = Duration::from_secs(10);
const SUMMARY_TAIL_CHARS: usize = 4000;
const TERM_COLS: u16 = 120;
const TERM_ROWS: u16 = 36;

#[derive(Debug)]
struct RpcError {
    code: i64,
    message: String,
}

#[derive(Clone, Debug)]
struct Report {
    status: String,
    summary: String,
}

/// One concrete execution of a node: the root, or a single delegated child.
pub struct Instance {
    pub id: String,
    pub node_id: String,
    /// Ancestor node ids, nearest last. Used for cycle detection.
    pub chain: Vec<String>,
    pub worktree: PathBuf,
    pub is_root: bool,
    session: Mutex<Option<String>>,
    report: Mutex<Option<oneshot::Sender<Report>>>,
}

impl Instance {
    fn new(id: String, node_id: String, chain: Vec<String>, worktree: PathBuf, is_root: bool) -> Self {
        Self {
            id,
            node_id,
            chain,
            worktree,
            is_root,
            session: Mutex::new(None),
            report: Mutex::new(None),
        }
    }

    fn arm_report(&self, sender: oneshot::Sender<Report>) {
        *lock(&self.report) = Some(sender);
    }

    /// Handles the injected `report_task_result` tool. Only the first call is accepted.
    fn report_back(&self, args: &Value) -> AppResult<String> {
        if self.is_root {
            crate::error::bail!("the root orchestrator has no report channel; answer the user directly");
        }
        let status = args.get("status").and_then(Value::as_str).unwrap_or("completed");
        if !matches!(status, "completed" | "blocked" | "failed") {
            crate::error::bail!("status must be one of completed, blocked, failed");
        }
        let summary = args.get("summary").and_then(Value::as_str).unwrap_or("").trim().to_string();
        let sender = lock(&self.report)
            .take()
            .ok_or_else(|| AppError::msg("a result was already reported for this task"))?;
        let _ = sender.send(Report {
            status: status.to_string(),
            summary,
        });
        Ok("Result recorded. Your session is ending; stop working now.".into())
    }
}

/// Everything one active run needs. Immutable topology snapshot plus runtime bookkeeping.
pub struct RunContext {
    pub run_id: String,
    pub repo_root: PathBuf,
    pub graph: OrchestrationGraph,
    pub goal: String,
    pub token: String,
    pub run_dir: PathBuf,
    pub root_node_id: String,
    pub cancel: CancellationToken,
    instances: DashMap<String, Arc<Instance>>,
    edge_attempts: DashMap<String, u32>,
    instance_counters: DashMap<String, u32>,
    approvals: DashMap<String, Vec<oneshot::Sender<bool>>>,
}

impl RunContext {
    fn new(
        run_id: String,
        repo_root: PathBuf,
        graph: OrchestrationGraph,
        goal: String,
        token: String,
        run_dir: PathBuf,
        root_node_id: String,
    ) -> Self {
        Self {
            run_id,
            repo_root,
            graph,
            goal,
            token,
            run_dir,
            root_node_id,
            cancel: CancellationToken::new(),
            instances: DashMap::new(),
            edge_attempts: DashMap::new(),
            instance_counters: DashMap::new(),
            approvals: DashMap::new(),
        }
    }

    fn short_id(&self) -> String {
        self.run_id.chars().take(8).collect()
    }

    fn instance(&self, instance_id: &str) -> AppResult<Arc<Instance>> {
        self.instances
            .get(instance_id)
            .map(|entry| entry.value().clone())
            .ok_or_else(|| AppError::msg(format!("unknown instance '{instance_id}'")))
    }

    /// Counts an attempt on `edge_id` and returns the new total (1-based).
    fn register_attempt(&self, edge_id: &str) -> u32 {
        let mut count = self.edge_attempts.entry(edge_id.to_string()).or_insert(0);
        *count += 1;
        *count
    }

    /// Children are numbered from 1; the root is always `<node>.0`.
    fn next_instance_no(&self, node_id: &str) -> u32 {
        let mut count = self.instance_counters.entry(node_id.to_string()).or_insert(0);
        *count += 1;
        *count
    }

    /// Opens a gate for `edge_id`. Must be called *before* the gate is announced, so an
    /// operator decision that arrives immediately after the announcement is never lost.
    fn register_approval(&self, edge_id: &str) -> oneshot::Receiver<bool> {
        let (sender, receiver) = oneshot::channel();
        self.approvals.entry(edge_id.to_string()).or_default().push(sender);
        receiver
    }

    async fn await_decision(&self, edge_id: &str, receiver: oneshot::Receiver<bool>) -> AppResult<bool> {
        tokio::select! {
            _ = self.cancel.cancelled() => {
                crate::error::bail!("run stopped while waiting for approval on edge '{edge_id}'")
            }
            decision = receiver => Ok(decision.unwrap_or(false)),
        }
    }
}

pub struct Router {
    events: Arc<dyn EventSink>,
    db: Arc<StateDb>,
    pty: Arc<PtyManager>,
    locks: Arc<RepoLocks>,
    port: u16,
    /// Executable that serves `--mcp-bridge` for provider CLIs. The app passes itself.
    bridge_exe: PathBuf,
    runs: DashMap<String, Arc<RunContext>>,
}

impl Router {
    pub fn new(
        events: Arc<dyn EventSink>,
        db: Arc<StateDb>,
        pty: Arc<PtyManager>,
        locks: Arc<RepoLocks>,
        port: u16,
        bridge_exe: PathBuf,
    ) -> Self {
        Self {
            events,
            db,
            pty,
            locks,
            port,
            bridge_exe,
            runs: DashMap::new(),
        }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn authorize(&self, run_id: &str, instance_id: &str, token: &str) -> AppResult<()> {
        let run = self.run(run_id)?;
        if !constant_time_eq(run.token.as_bytes(), token.as_bytes()) {
            crate::error::bail!("bad token");
        }
        run.instance(instance_id).map(|_| ())
    }

    fn run(&self, run_id: &str) -> AppResult<Arc<RunContext>> {
        self.runs
            .get(run_id)
            .map(|entry| entry.value().clone())
            .ok_or_else(|| AppError::msg(format!("run '{run_id}' is not active")))
    }

    /// Dispatches one JSON-RPC message. Returns `None` for notifications.
    pub async fn handle_message(&self, run_id: &str, instance_id: &str, message: Value) -> AppResult<Option<Value>> {
        let Some(id) = message.get("id").cloned() else {
            return Ok(None);
        };
        let run = self.run(run_id)?;
        let instance = run.instance(instance_id)?;
        let method = message.get("method").and_then(Value::as_str).unwrap_or_default();

        let outcome: Result<Value, RpcError> = match method {
            "initialize" => {
                let requested = message
                    .pointer("/params/protocolVersion")
                    .and_then(Value::as_str)
                    .unwrap_or(PROTOCOL_VERSION);
                Ok(json!({
                    "protocolVersion": requested,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": "openconductor", "version": env!("CARGO_PKG_VERSION") }
                }))
            }
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({
                "tools": compiler::tool_descriptors(&run.graph, &instance.node_id, !instance.is_root)
            })),
            "tools/call" => {
                let params = message.get("params").cloned().unwrap_or(Value::Null);
                self.call_tool(&run, &instance, params).await
            }
            other => Err(RpcError {
                code: -32601,
                message: format!("method not found: {other}"),
            }),
        };

        Ok(Some(match outcome {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err(e) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": e.code, "message": e.message } }),
        }))
    }

    async fn call_tool(&self, run: &Arc<RunContext>, instance: &Arc<Instance>, params: Value) -> Result<Value, RpcError> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError {
                code: -32602,
                message: "tools/call requires a tool name".into(),
            })?
            .to_string();
        let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
        self.log(
            Some(run.as_ref()),
            Some(instance.node_id.as_str()),
            "mcp_call",
            json!({ "tool": name, "instance": instance.id }),
        );

        let result: AppResult<String> = if name == REPORT_TOOL {
            instance.report_back(&args)
        } else if let Some(target) = name.strip_prefix("delegate_to_") {
            self.dispatch(run, instance, RelationType::DelegatesTo, target, &args).await
        } else if let Some(target) = name.strip_prefix("verify_with_") {
            self.dispatch(run, instance, RelationType::Verifies, target, &args).await
        } else if let Some(target) = name.strip_prefix("escalate_to_") {
            self.dispatch(run, instance, RelationType::EscalatesTo, target, &args).await
        } else {
            Err(AppError::msg(format!("unknown tool '{name}'")))
        };

        // Tool failures are reported inside the result (isError) so the calling model can
        // react; they are not protocol errors.
        Ok(match result {
            Ok(text) => json!({ "content": [{ "type": "text", "text": text }], "isError": false }),
            Err(err) => json!({ "content": [{ "type": "text", "text": err.to_string() }], "isError": true }),
        })
    }

    async fn dispatch(
        &self,
        run: &Arc<RunContext>,
        parent: &Arc<Instance>,
        relation: RelationType,
        target_id: &str,
        args: &Value,
    ) -> AppResult<String> {
        let edge = run
            .graph
            .edges
            .iter()
            .find(|e| e.source == parent.node_id && e.target == target_id && e.data.relation_type == relation)
            .cloned()
            .ok_or_else(|| {
                AppError::msg(format!(
                    "no {} edge from '{}' to '{}' in the topology",
                    relation_label(relation),
                    parent.node_id,
                    target_id
                ))
            })?;
        let target = run
            .graph
            .nodes
            .iter()
            .find(|n| n.id == target_id)
            .cloned()
            .ok_or_else(|| AppError::msg(format!("node '{target_id}' does not exist")))?;

        check_hop(&parent.chain, &parent.node_id, target_id, relation)?;
        let attempt = run.register_attempt(&edge.id);
        if attempt > edge.data.max_retries {
            crate::error::bail!(
                "edge '{}' exhausted its maxRetries budget ({})",
                edge.id,
                edge.data.max_retries
            );
        }
        let prompt = compose_prompt(relation, &parent.node_id, args)?;

        if edge.data.requires_approval {
            let decision = run.register_approval(&edge.id);
            self.set_status(run, target_id, NodeStatus::WaitingApproval, None, None);
            self.events.emit(
                &format!("approval-request://{}", run.run_id),
                json!({
                    "edgeId": edge.id,
                    "fromNodeId": parent.node_id,
                    "targetNodeId": target_id,
                    "prompt": prompt,
                }),
            );
            let approved = run.await_decision(&edge.id, decision).await?;
            if !approved {
                self.set_status(run, target_id, NodeStatus::Idle, None, None);
                crate::error::bail!("the operator rejected this handoff on edge '{}'", edge.id);
            }
        }

        let instance_no = run.next_instance_no(target_id);
        let instance_id = format!("{target_id}.{instance_no}");
        let worktree = self.provision_worktree(run, parent, &target, instance_no).await?;

        let chain: Vec<String> = parent
            .chain
            .iter()
            .cloned()
            .chain(std::iter::once(parent.node_id.clone()))
            .collect();
        let instance = Arc::new(Instance::new(
            instance_id.clone(),
            target_id.to_string(),
            chain,
            worktree.clone(),
            false,
        ));
        let (report_tx, report_rx) = oneshot::channel();
        instance.arm_report(report_tx);
        run.instances.insert(instance_id.clone(), instance.clone());

        let system = compiler::instructions(&run.graph, &target, &run.repo_root, true);
        let invocation = self.build_invocation(run, &target, Mode::Headless, &system, &prompt, &instance_id)?;
        let session = self.spawn_session(run, &instance, invocation, &worktree)?;
        self.set_status(run, target_id, NodeStatus::Running, None, Some(session.id.clone()));

        let outcome = self.await_outcome(run, &session, report_rx).await;
        let stat = diff_stat(&worktree).await;
        let status = if matches!(outcome.status.as_str(), "failed" | "blocked") {
            NodeStatus::Error
        } else {
            NodeStatus::Idle
        };
        self.set_status(run, target_id, status, Some(excerpt(&outcome.summary, 200)), Some(session.id.clone()));

        Ok(json!({
            "node": target_id,
            "instance": instance_id,
            "attempt": attempt,
            "status": outcome.status,
            "exitCode": outcome.exit_code,
            "summary": outcome.summary,
            "diffStat": stat,
            "worktree": worktree.to_string_lossy(),
            "sessionId": session.id,
        })
        .to_string())
    }

    /// Shared nodes reuse the caller's worktree so edits are visible live. Isolated nodes
    /// get a child worktree branched from the caller's last commit.
    async fn provision_worktree(
        &self,
        run: &RunContext,
        parent: &Instance,
        target: &GraphNode,
        instance_no: u32,
    ) -> AppResult<PathBuf> {
        match target.data.worktree_mode {
            WorktreeMode::Shared => Ok(parent.worktree.clone()),
            WorktreeMode::Isolated => {
                let short = run.short_id();
                let folder = format!("{short}-{}-{instance_no}", target.id);
                let branch = format!("oc/{short}/{}-{instance_no}", target.id);
                let head = cmd::run(&parent.worktree, &["rev-parse", "HEAD"])
                    .await
                    .ok()
                    .map(|sha| sha.trim().to_string());
                let info = worktree::create_worktree(&self.locks, &run.repo_root, &branch, &folder, head.as_deref()).await?;
                Ok(PathBuf::from(info.path))
            }
        }
    }

    fn build_invocation(
        &self,
        run: &RunContext,
        node: &GraphNode,
        mode: Mode,
        system: &str,
        prompt: &str,
        instance_id: &str,
    ) -> AppResult<Invocation> {
        let bridge = BridgeAddress {
            exe: &self.bridge_exe,
            port: self.port,
            run_id: &run.run_id,
            instance_id,
            token: &run.token,
        };
        compiler::invocation(
            node.data.provider,
            mode,
            &node.data.model,
            system,
            prompt,
            &bridge,
            &run.run_dir,
        )
    }

    fn spawn_session(
        &self,
        run: &RunContext,
        instance: &Instance,
        invocation: Invocation,
        cwd: &Path,
    ) -> AppResult<Arc<PtySession>> {
        let program = resolver::resolve_provider(&invocation.program).ok_or_else(|| {
            AppError::msg(format!(
                "the '{}' CLI was not found on your PATH. Install it or fix your shell profile, then retry.",
                invocation.program
            ))
        })?;
        let session_id = Uuid::new_v4().to_string();
        self.db.record_pty_session(
            &session_id,
            Some(&run.run_id),
            Some(&instance.node_id),
            cwd,
            &invocation.program,
            now_ms(),
        )?;
        let session = self.pty.spawn(SpawnSpec {
            session_id: session_id.clone(),
            cwd: cwd.to_path_buf(),
            program: program.to_string_lossy().into_owned(),
            args: invocation.args,
            env: invocation.env,
            cols: TERM_COLS,
            rows: TERM_ROWS,
        })?;
        *lock(&instance.session) = Some(session_id);

        Ok(session)
    }

    /// Blocks until the child reports or exits. The caller's `tools/call` stays pending
    /// for exactly this long.
    async fn await_outcome(
        &self,
        run: &RunContext,
        session: &Arc<PtySession>,
        report_rx: oneshot::Receiver<Report>,
    ) -> Outcome {
        let mut exit_rx = session.subscribe_exit();
        let mut report_rx = report_rx;
        let mut report_open = true;
        let mut report: Option<Report> = None;

        loop {
            tokio::select! {
                _ = run.cancel.cancelled() => {
                    self.terminate(session).await;
                    return resolve_outcome(None, session.exit_code(), true, &session.history());
                }
                received = &mut report_rx, if report_open => {
                    report_open = false;
                    if let Ok(found) = received {
                        report = Some(found);
                        break;
                    }
                }
                code = wait_for_exit(&mut exit_rx) => {
                    return resolve_outcome(report, Some(code), false, &session.history());
                }
            }
        }

        // The child reported: give it a moment to exit cleanly, then tear it down.
        match tokio::time::timeout(REPORT_GRACE, wait_for_exit(&mut exit_rx)).await {
            Ok(code) => resolve_outcome(report, Some(code), false, &session.history()),
            Err(_) => {
                self.terminate(session).await;
                resolve_outcome(report, session.exit_code(), false, &session.history())
            }
        }
    }

    async fn terminate(&self, session: &PtySession) {
        let pty = self.pty.clone();
        let id = session.id.clone();
        let _ = tokio::task::spawn_blocking(move || pty.terminate(&id)).await;
    }

    /// Starts a run: records it, spawns the interactive root, and returns the run id.
    pub async fn start_run(
        self: &Arc<Self>,
        repo_root: PathBuf,
        graph: OrchestrationGraph,
        root_node_id: String,
        goal: String,
    ) -> AppResult<String> {
        let goal = goal.trim().to_string();
        if goal.is_empty() {
            crate::error::bail!("Describe the goal for the orchestrator before running");
        }
        let root = graph
            .nodes
            .iter()
            .find(|n| n.id == root_node_id)
            .cloned()
            .ok_or_else(|| AppError::msg(format!("root node '{root_node_id}' does not exist")))?;

        let run_id = Uuid::new_v4().to_string();
        let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let run = Arc::new(RunContext::new(
            run_id.clone(),
            repo_root.clone(),
            graph,
            goal.clone(),
            token,
            compiler::run_dir(&run_id),
            root_node_id.clone(),
        ));
        std::fs::create_dir_all(&run.run_dir)?;
        self.db
            .insert_run(&run_id, &repo_root.to_string_lossy(), &root_node_id, &goal, now_ms())?;

        let root_instance = Arc::new(Instance::new(
            format!("{root_node_id}.0"),
            root_node_id.clone(),
            vec![],
            repo_root.clone(),
            true,
        ));
        run.instances.insert(root_instance.id.clone(), root_instance.clone());

        let system = compiler::instructions(&run.graph, &root, &run.repo_root, false);
        let prompt = format!("# Goal\n{goal}");
        let invocation = self.build_invocation(&run, &root, Mode::Interactive, &system, &prompt, &root_instance.id)?;
        let session = self.spawn_session(&run, &root_instance, invocation, &repo_root)?;

        self.runs.insert(run_id.clone(), run.clone());
        self.set_status(&run, &root_node_id, NodeStatus::Running, None, Some(session.id.clone()));

        // The root is an interactive TUI: when the user exits it, the node goes idle again.
        let router = self.clone();
        let mut exit_rx = session.subscribe_exit();
        tauri::async_runtime::spawn(async move {
            let code = wait_for_exit(&mut exit_rx).await;
            let status = if code == 0 { NodeStatus::Idle } else { NodeStatus::Error };
            router.set_status(&run, &root_node_id, status, None, None);
        });
        Ok(run_id)
    }

    pub async fn stop_run(&self, run_id: &str) -> AppResult<()> {
        let (_, run) = self
            .runs
            .remove(run_id)
            .ok_or_else(|| AppError::msg(format!("run '{run_id}' is not active")))?;
        run.cancel.cancel();

        let mut session_ids = Vec::new();
        for entry in run.instances.iter() {
            if let Some(id) = lock(&entry.value().session).clone() {
                session_ids.push(id);
            }
        }
        for session_id in session_ids {
            let pty = self.pty.clone();
            let _ = tokio::task::spawn_blocking(move || pty.terminate(&session_id)).await;
        }
        for node in &run.graph.nodes {
            self.set_status(&run, &node.id, NodeStatus::Idle, None, None);
        }
        self.db.finish_run(run_id, "stopped", now_ms())?;
        let _ = std::fs::remove_dir_all(&run.run_dir);
        Ok(())
    }

    pub fn resolve_approval(&self, run_id: &str, edge_id: &str, approved: bool) -> AppResult<()> {
        let run = self.run(run_id)?;
        let waiting = run
            .approvals
            .remove(edge_id)
            .map(|(_, senders)| senders)
            .unwrap_or_default();
        if waiting.is_empty() {
            crate::error::bail!("no handoff is waiting for approval on edge '{edge_id}'");
        }
        for sender in waiting {
            let _ = sender.send(approved);
        }
        self.log(Some(run.as_ref()), None, "approval", json!({ "edgeId": edge_id, "approved": approved }));
        Ok(())
    }

    /// Cancels every active run. Process teardown is handled by `PtyManager::terminate_all`.
    pub fn shutdown(&self) {
        for entry in self.runs.iter() {
            entry.value().cancel.cancel();
        }
    }

    fn set_status(
        &self,
        run: &RunContext,
        node_id: &str,
        status: NodeStatus,
        summary: Option<String>,
        session_id: Option<String>,
    ) {
        let label = serde_json::to_value(status)
            .ok()
            .and_then(|value| value.as_str().map(str::to_string))
            .unwrap_or_else(|| "idle".into());
        self.events.emit(
            &format!("node-status://{node_id}"),
            json!({
                "runId": run.run_id,
                "status": label,
                "summary": summary,
                "sessionId": session_id,
            }),
        );
        // Journaled with the session id so a late-attaching UI can recover it from history.
        self.log(
            Some(run),
            Some(node_id),
            "status",
            json!({ "status": label, "sessionId": session_id }),
        );
    }

    fn log(&self, run: Option<&RunContext>, node_id: Option<&str>, kind: &str, payload: Value) {
        let Some(run) = run else { return };
        let ts = now_ms();
        if let Err(e) = self.db.insert_event(&run.run_id, node_id, kind, &payload, ts) {
            eprintln!("failed to persist run event: {e}");
        }
        self.events.emit(
            &format!("run-log://{}", run.run_id),
            json!({ "ts": ts, "nodeId": node_id, "kind": kind, "payload": payload }),
        );
    }
}

/// Final result of one delegated execution.
#[derive(Debug, PartialEq)]
pub struct Outcome {
    pub status: String,
    pub exit_code: Option<i32>,
    pub summary: String,
}

/// Pure outcome resolution: an explicit report wins, otherwise the exit code decides.
/// Output is taken from the stream-json result line when present, else from the tail.
fn resolve_outcome(report: Option<Report>, exit_code: Option<i32>, cancelled: bool, history: &str) -> Outcome {
    if cancelled {
        return Outcome {
            status: "cancelled".into(),
            exit_code,
            summary: final_text(history),
        };
    }
    if let Some(report) = report {
        return Outcome {
            status: report.status,
            exit_code,
            summary: report.summary,
        };
    }
    let status = if exit_code == Some(0) { "completed" } else { "failed" };
    Outcome {
        status: status.into(),
        exit_code,
        summary: final_text(history),
    }
}

/// Enforces acyclic delegation and escalation chains. Verification edges may loop (the
/// Senior Dev <-> Debugger pattern); they are bounded by the edge attempt budget instead.
pub fn check_hop(chain: &[String], from: &str, target: &str, relation: RelationType) -> AppResult<()> {
    if relation == RelationType::Verifies {
        return Ok(());
    }
    if from == target || chain.iter().any(|node| node == target) {
        let path = chain.iter().map(String::as_str).chain([from, target]).collect::<Vec<_>>().join(" -> ");
        crate::error::bail!("cycle detected: {path}");
    }
    Ok(())
}

fn compose_prompt(relation: RelationType, from: &str, args: &Value) -> AppResult<String> {
    let text = |key: &str| {
        args.get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string()
    };
    let or_none = |value: String, fallback: &str| if value.is_empty() { fallback.to_string() } else { value };

    match relation {
        RelationType::DelegatesTo => {
            let task = text("task");
            if task.is_empty() {
                crate::error::bail!("'task' is required");
            }
            let files = args
                .get("target_files")
                .and_then(Value::as_array)
                .map(|items| items.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "))
                .unwrap_or_default();
            Ok(format!(
                "## Delegated task (from `{from}`)\n{task}\n\n## Acceptance criteria\n{}\n\n## Target files\n{}",
                or_none(text("acceptance_criteria"), "(none specified)"),
                or_none(files, "(none specified)")
            ))
        }
        RelationType::Verifies => {
            let summary = text("changes_summary");
            if summary.is_empty() {
                crate::error::bail!("'changes_summary' is required");
            }
            Ok(format!(
                "## Verification request (from `{from}`)\n### Changes\n{summary}\n\n### Test command\n{}\n\nVerify these changes: run the test command, investigate failures, and report pass/fail with evidence.",
                or_none(text("test_command"), "(none given: choose the appropriate verification)")
            ))
        }
        RelationType::EscalatesTo => {
            let blocker = text("blocker_description");
            if blocker.is_empty() {
                crate::error::bail!("'blocker_description' is required");
            }
            Ok(format!(
                "## Escalation (from `{from}`)\n### Blocker\n{blocker}\n\n### Context\n{}\n\nDecide how to unblock this within your role, act if needed, and report your decision.",
                or_none(text("context"), "(none provided)")
            ))
        }
    }
}

fn relation_label(relation: RelationType) -> &'static str {
    match relation {
        RelationType::DelegatesTo => "delegates_to",
        RelationType::Verifies => "verifies",
        RelationType::EscalatesTo => "escalates_to",
    }
}

async fn diff_stat(worktree: &Path) -> String {
    let has_head = cmd::raw(worktree, &["rev-parse", "--verify", "-q", "HEAD"])
        .await
        .map(|output| output.status.success())
        .unwrap_or(false);
    let base = if has_head { "HEAD" } else { EMPTY_TREE };
    cmd::run(worktree, &["diff", "--stat", base])
        .await
        .unwrap_or_default()
        .trim()
        .to_string()
}

async fn wait_for_exit(receiver: &mut watch::Receiver<Option<i32>>) -> i32 {
    loop {
        let current = *receiver.borrow_and_update();
        if let Some(code) = current {
            return code;
        }
        if receiver.changed().await.is_err() {
            return -1;
        }
    }
}

/// Extracts the agent's final answer. Claude's stream-json emits a `result` record; other
/// CLIs fall back to the last few thousand characters of their output.
pub fn final_text(raw: &str) -> String {
    let clean = strip_ansi(raw);
    for line in clean.lines().rev() {
        let trimmed = line.trim();
        if !trimmed.starts_with('{') {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
            if value.get("type").and_then(Value::as_str) == Some("result") {
                if let Some(text) = value.get("result").and_then(Value::as_str) {
                    return text.to_string();
                }
            }
        }
    }
    excerpt_tail(clean.trim(), SUMMARY_TAIL_CHARS)
}

fn excerpt_tail(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_string();
    }
    text.chars().skip(count - max_chars).collect()
}

fn excerpt(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        text.to_string()
    } else {
        let head: String = text.chars().take(max_chars).collect();
        format!("{head}…")
    }
}

/// Removes CSI, OSC and two-byte escape sequences from terminal output.
pub fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.peek().copied() {
            Some('[') => {
                chars.next();
                for next in chars.by_ref() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
            }
            Some(']') => {
                chars.next();
                while let Some(next) = chars.next() {
                    if next == '\u{7}' {
                        break;
                    }
                    if next == '\u{1b}' {
                        chars.next();
                        break;
                    }
                }
            }
            Some(_) => {
                chars.next();
            }
            None => {}
        }
    }
    out
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_report_wins_over_exit_code() {
        let report = Report {
            status: "blocked".into(),
            summary: "needs API key".into(),
        };
        let outcome = resolve_outcome(Some(report), Some(0), false, "");
        assert_eq!(outcome.status, "blocked");
        assert_eq!(outcome.summary, "needs API key");
    }

    #[test]
    fn exit_code_decides_without_report() {
        assert_eq!(resolve_outcome(None, Some(0), false, "done").status, "completed");
        assert_eq!(resolve_outcome(None, Some(1), false, "boom").status, "failed");
        assert_eq!(resolve_outcome(None, None, true, "").status, "cancelled");
    }

    #[test]
    fn extracts_claude_stream_json_result() {
        let raw = "{\"type\":\"system\",\"subtype\":\"init\"}\r\n{\"type\":\"assistant\"}\r\n{\"type\":\"result\",\"result\":\"Fixed the parser.\"}\r\n";
        assert_eq!(final_text(raw), "Fixed the parser.");
    }

    #[test]
    fn falls_back_to_ansi_stripped_tail() {
        let raw = "\u{1b}[32mall tests passed\u{1b}[0m";
        assert_eq!(final_text(raw), "all tests passed");
    }

    #[test]
    fn cycle_guard_blocks_back_edges_but_allows_verification_loops() {
        let chain = vec!["ceo".to_string()];
        assert!(check_hop(&chain, "senior-dev", "ceo", RelationType::DelegatesTo).is_err());
        assert!(check_hop(&chain, "senior-dev", "senior-dev", RelationType::Verifies).is_ok());
        assert!(check_hop(&chain, "ceo", "senior-dev", RelationType::DelegatesTo).is_ok());
    }

    #[test]
    fn delegate_prompt_requires_task() {
        assert!(compose_prompt(RelationType::DelegatesTo, "ceo", &json!({})).is_err());
        let prompt = compose_prompt(RelationType::DelegatesTo, "ceo", &json!({"task": "add login", "target_files": ["src/a.ts"]})).unwrap();
        assert!(prompt.contains("add login"));
        assert!(prompt.contains("src/a.ts"));
    }

    #[test]
    fn constant_time_compare_matches_equality() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }
}
