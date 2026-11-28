//! Deterministic handoff state machine (HANDOFF §4.4.4).
//!
//! The root orchestrator runs interactively in a PTY. When it calls a `delegate_to_*`,
//! `verify_with_*` or `escalate_to_*` tool, the router:
//!   1. validates the edge exists and that the hop does not form a forbidden cycle,
//!   2. enforces the attempt budget of the task it belongs to (see [`TaskStream`]),
//!   3. pauses on an approval gate when `requiresApproval` is set,
//!   4. provisions the target's worktree (shared with the caller, or an isolated child),
//!   5. spawns the target headless in a visible PTY with an injected `report_task_result`,
//!   6. blocks the caller's `tools/call` until the child reports or exits. A `completed` report
//!      only stands if the repository's gates pass (see `crate::gates`). The caller gets the
//!      summary, exit code, gate results and `git diff --stat`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use dashmap::DashMap;
use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::{oneshot, watch};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::compiler::{self, BridgeAddress, Invocation, Mode, REPORT_TOOL};
use crate::env::resolver;
use crate::error::{AppError, AppResult};
use crate::gates::{self, Gate, GateResult};
use crate::git::{cmd, diff_engine::EMPTY_TREE, locks::RepoLocks, worktree};
use crate::graph::{AccessLevel, GraphNode, NodeStatus, OrchestrationGraph, RelationType, WorktreeMode};
use crate::pty::{PtyManager, PtySession, SpawnSpec};
use crate::runtime::{events::EventSink, now_ms, state_db::StateDb};

const PROTOCOL_VERSION: &str = "2025-06-18";
/// After a child reports, how long it may keep running before we terminate it.
const REPORT_GRACE: Duration = Duration::from_secs(10);
const SUMMARY_TAIL_CHARS: usize = 4000;
/// Longest prompt or result journaled per handoff, so the run history stays readable and bounded.
const JOURNAL_TEXT_CHARS: usize = 4000;
const TERM_COLS: u16 = 120;
const TERM_ROWS: u16 = 36;

#[derive(Debug)]
struct RpcError {
    code: i64,
    message: String,
}

/// What a child reported. `status` and `summary` are the agent's claim. `detail` is its evidence.
#[derive(Clone, Debug)]
struct Report {
    status: String,
    summary: String,
    detail: ReportDetail,
}

/// The structured part of a report. The runtime routes on `status` and `failure_signature`. The
/// rest is evidence for the run log and the caller.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportDetail {
    pub changed_files: Vec<String>,
    /// Each item is `{ "command", "exitCode", "excerpt" }`, checked when the report is read.
    pub commands_run: Vec<Value>,
    pub open_questions: Vec<String>,
    pub failure_signature: Option<String>,
}

const MAX_REPORT_ITEMS: usize = 200;
const MAX_REPORT_TEXT_CHARS: usize = 2000;
const MAX_COMMAND_CHARS: usize = 500;
const MAX_SIGNATURE_CHARS: usize = 200;

/// Reads a child's report. The fields the runtime acts on, and the shape of the evidence, are
/// checked. A bad report is refused as a tool error before anything is recorded, so the agent can
/// correct it and report again.
fn parse_report(args: &Value) -> AppResult<Report> {
    let status = args.get("status").and_then(Value::as_str).unwrap_or("completed");
    if !matches!(status, "completed" | "blocked" | "failed") {
        crate::error::bail!("status must be one of completed, blocked, failed");
    }
    let summary = args.get("summary").and_then(Value::as_str).unwrap_or("").trim().to_string();
    let detail = ReportDetail {
        changed_files: string_list(args, "changed_files")?,
        commands_run: commands_list(args)?,
        open_questions: string_list(args, "open_questions")?,
        failure_signature: failure_signature_text(args)?,
    };
    Ok(Report {
        status: status.to_string(),
        summary,
        detail,
    })
}

fn string_list(args: &Value, key: &str) -> AppResult<Vec<String>> {
    let Some(value) = args.get(key) else {
        return Ok(Vec::new());
    };
    let items = value
        .as_array()
        .ok_or_else(|| AppError::msg(format!("'{key}' must be an array of strings")))?;
    if items.len() > MAX_REPORT_ITEMS {
        crate::error::bail!("'{key}' has more than {MAX_REPORT_ITEMS} items");
    }
    items
        .iter()
        .map(|item| {
            item.as_str()
                .map(|text| excerpt(text, MAX_REPORT_TEXT_CHARS))
                .ok_or_else(|| AppError::msg(format!("'{key}' must be an array of strings")))
        })
        .collect()
}

fn commands_list(args: &Value) -> AppResult<Vec<Value>> {
    let Some(value) = args.get("commands_run") else {
        return Ok(Vec::new());
    };
    let items = value
        .as_array()
        .ok_or_else(|| AppError::msg("'commands_run' must be an array"))?;
    if items.len() > MAX_REPORT_ITEMS {
        crate::error::bail!("'commands_run' has more than {MAX_REPORT_ITEMS} items");
    }
    items
        .iter()
        .map(|item| -> AppResult<Value> {
            let command = item
                .get("command")
                .and_then(Value::as_str)
                .filter(|text| !text.trim().is_empty())
                .ok_or_else(|| AppError::msg("each commands_run item needs a 'command' string"))?;
            let exit_code = match item.get("exit_code") {
                None | Some(Value::Null) => Value::Null,
                Some(code) => match code.as_i64() {
                    Some(number) => json!(number),
                    None => crate::error::bail!("'exit_code' must be an integer"),
                },
            };
            let output = item.get("excerpt").and_then(Value::as_str).unwrap_or("");
            Ok(json!({
                "command": excerpt(command, MAX_COMMAND_CHARS),
                "exitCode": exit_code,
                "excerpt": excerpt(output, MAX_REPORT_TEXT_CHARS),
            }))
        })
        .collect()
}

fn failure_signature_text(args: &Value) -> AppResult<Option<String>> {
    match args.get("failure_signature") {
        None | Some(Value::Null) => Ok(None),
        Some(value) => {
            let text = value
                .as_str()
                .ok_or_else(|| AppError::msg("'failure_signature' must be a string"))?
                .trim();
            Ok((!text.is_empty()).then(|| excerpt(&gates::normalize(text), MAX_SIGNATURE_CHARS)))
        }
    }
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
        // Validate first: a refused report leaves the channel open, so the agent can fix it.
        let report = parse_report(args)?;
        let sender = lock(&self.report)
            .take()
            .ok_or_else(|| AppError::msg("a result was already reported for this task"))?;
        let _ = sender.send(report);
        Ok("Result recorded. Your session is ending; stop working now.".into())
    }
}

/// Where a task stands on its edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskPhase {
    /// Attempts may continue.
    Open,
    /// A named task whose work passed its gates. Retrying it is refused: new work needs a new task id.
    Accepted,
    /// The same failure came back twice in a row. Another attempt would repeat it, so the task
    /// waits for someone to decide how to proceed.
    Stalled,
}

/// The attempts made on one task through one edge. A task is named by its `task_id` argument.
/// Calls without one share a stream per calling instance and edge, so retries that leave the id
/// out still count against a budget and cannot loop without end.
#[derive(Debug)]
pub struct TaskStream {
    pub named: bool,
    pub attempts: u32,
    pub last_signature: Option<String>,
    pub phase: TaskPhase,
}

impl TaskStream {
    pub fn new(named: bool) -> Self {
        Self {
            named,
            attempts: 0,
            last_signature: None,
            phase: TaskPhase::Open,
        }
    }
}

/// The key a call's attempts are counted under.
pub fn stream_key(edge_id: &str, caller_instance: &str, task_id: Option<&str>) -> String {
    match task_id {
        Some(id) => format!("{edge_id}#{id}"),
        None => format!("{edge_id}@{caller_instance}"),
    }
}

/// Counts an attempt on the stream, or refuses it. A refused attempt is not counted.
pub fn begin_attempt(stream: &mut TaskStream, max_retries: u32, label: &str) -> AppResult<u32> {
    match stream.phase {
        TaskPhase::Accepted => crate::error::bail!(
            "task '{label}' was already accepted. Start new work with a new task_id instead of retrying it."
        ),
        TaskPhase::Stalled => crate::error::bail!(
            "task '{label}' stalled: the same failure came back twice. Escalate or re-scope it instead of trying it again."
        ),
        TaskPhase::Open => {}
    }
    if stream.attempts >= max_retries {
        crate::error::bail!(
            "task '{label}' exhausted its maxRetries budget ({max_retries}). Escalate or summarize the blocker instead of retrying."
        );
    }
    stream.attempts += 1;
    Ok(stream.attempts)
}

/// Records how an attempt ended. A pass closes a named task. A failure whose signature repeats the
/// one before it stalls the task.
pub fn settle_attempt(stream: &mut TaskStream, passed: bool, signature: Option<&str>) {
    if passed {
        stream.last_signature = None;
        if stream.named {
            stream.phase = TaskPhase::Accepted;
        }
        return;
    }
    let repeated = matches!(
        (signature, stream.last_signature.as_deref()),
        (Some(new), Some(previous)) if new == previous
    );
    stream.last_signature = signature.map(str::to_string);
    if repeated {
        stream.phase = TaskPhase::Stalled;
    }
}

/// Short ids made of characters that are safe in a stream key, a log line and a branch name.
fn valid_task_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The optional `task_id` a call names its task with.
fn task_id_arg(args: &Value) -> AppResult<Option<String>> {
    match args.get("task_id") {
        None | Some(Value::Null) => Ok(None),
        Some(value) => {
            let id = value
                .as_str()
                .map(str::trim)
                .ok_or_else(|| AppError::msg("'task_id' must be a string"))?;
            if id.is_empty() {
                return Ok(None);
            }
            if !valid_task_id(id) {
                crate::error::bail!("'task_id' must be up to 64 letters, digits, '-' or '_'");
            }
            Ok(Some(id.to_string()))
        }
    }
}

/// What landing an isolated agent's work on the integration branch did.
#[derive(Debug, PartialEq)]
enum MergeOutcome {
    Merged { branch: String, commit: String },
    NothingToMerge,
    Conflict { branch: String, files: Vec<String> },
}

/// Holds an instance's place in `RunContext::active` while it runs. The place is released when the
/// guard drops, including when the caller goes away and the dispatch future is dropped with it.
struct ActiveRegistration {
    run: Arc<RunContext>,
    instance_id: String,
}

impl ActiveRegistration {
    fn new(run: Arc<RunContext>, instance_id: String, worktree: PathBuf, writes: bool, parent: String) -> Self {
        run.active.insert(instance_id.clone(), ActiveInstance { worktree, writes, parent });
        Self { run, instance_id }
    }
}

impl Drop for ActiveRegistration {
    fn drop(&mut self) {
        self.run.active.remove(&self.instance_id);
    }
}

/// Where a planned task stands. `Pending` covers both not started and open for another attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PlanState {
    Pending,
    Running,
    Accepted,
    Stalled,
}

/// One task in the plan the root orchestrator wrote for the run.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannedTask {
    pub id: String,
    pub title: String,
    pub depends_on: Vec<String>,
    pub state: PlanState,
}

const MAX_PLAN_TASKS: usize = 100;
const MAX_PLAN_TITLE_CHARS: usize = 200;

/// One task as the root wrote it, before it is merged into the plan.
type PlanItem = (String, String, Vec<String>);

/// Reads a plan. Ids are well formed and unique, every dependency names a task in the plan, and the
/// dependencies form no cycle, so the plan can always be followed to its end.
pub fn parse_plan(args: &Value) -> AppResult<Vec<PlanItem>> {
    let items = args
        .get("tasks")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::msg("'tasks' must be an array"))?;
    if items.is_empty() || items.len() > MAX_PLAN_TASKS {
        crate::error::bail!("a plan needs between 1 and {MAX_PLAN_TASKS} tasks");
    }
    let mut plan: Vec<PlanItem> = Vec::new();
    for item in items {
        let id = item.get("id").and_then(Value::as_str).map(str::trim).unwrap_or("");
        if !valid_task_id(id) {
            crate::error::bail!("each planned task needs an 'id' of up to 64 letters, digits, '-' or '_'");
        }
        if plan.iter().any(|(existing, _, _)| existing == id) {
            crate::error::bail!("task id '{id}' is planned twice");
        }
        let title = item
            .get("title")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .ok_or_else(|| AppError::msg(format!("task '{id}' needs a 'title'")))?;
        let depends_on: Vec<String> = match item.get("depends_on") {
            None | Some(Value::Null) => Vec::new(),
            Some(value) => value
                .as_array()
                .ok_or_else(|| AppError::msg(format!("'depends_on' of task '{id}' must be an array")))?
                .iter()
                .map(|dep| {
                    dep.as_str()
                        .map(|text| text.trim().to_string())
                        .ok_or_else(|| AppError::msg(format!("'depends_on' of task '{id}' must be task ids")))
                })
                .collect::<AppResult<_>>()?,
        };
        plan.push((id.to_string(), excerpt(title, MAX_PLAN_TITLE_CHARS), depends_on));
    }
    for (id, _, depends_on) in &plan {
        if depends_on.iter().any(|dep| dep == id) {
            crate::error::bail!("task '{id}' depends on itself");
        }
        if let Some(missing) = depends_on.iter().find(|dep| !plan.iter().any(|(other, _, _)| other == *dep)) {
            crate::error::bail!("task '{id}' depends on '{missing}', which is not in the plan");
        }
    }
    // Kahn's algorithm: every task must be reachable once the tasks it depends on are placed.
    let mut placed: Vec<&str> = Vec::new();
    while placed.len() < plan.len() {
        let next = plan.iter().find(|(id, _, deps)| {
            !placed.contains(&id.as_str()) && deps.iter().all(|dep| placed.contains(&dep.as_str()))
        });
        match next {
            Some((id, _, _)) => placed.push(id.as_str()),
            None => crate::error::bail!("the plan's dependencies form a cycle"),
        }
    }
    Ok(plan)
}

/// The plan after the root replaces it with `items`. A task that has started keeps its state and
/// cannot be dropped, so a replan never rewrites history.
pub fn merge_plan(old: &[PlannedTask], items: Vec<PlanItem>) -> AppResult<Vec<PlannedTask>> {
    for task in old {
        let kept = items.iter().any(|(id, _, _)| *id == task.id);
        if !kept && task.state != PlanState::Pending {
            crate::error::bail!("task '{}' has started and cannot be dropped from the plan", task.id);
        }
    }
    Ok(items
        .into_iter()
        .map(|(id, title, depends_on)| {
            let state = old.iter().find(|task| task.id == id).map(|task| task.state).unwrap_or(PlanState::Pending);
            PlannedTask {
                id,
                title,
                depends_on,
                state,
            }
        })
        .collect())
}

/// Why a named task may not start now, or `None` when it may. A task must be in the plan, and every
/// task it depends on must be accepted.
pub fn plan_blocker(plan: &[PlannedTask], task_id: &str) -> Option<String> {
    let Some(task) = plan.iter().find(|task| task.id == task_id) else {
        return Some(format!("task '{task_id}' is not in the plan. Plan it with plan_tasks first."));
    };
    let waiting: Vec<&str> = task
        .depends_on
        .iter()
        .filter(|dep| {
            plan.iter()
                .find(|other| other.id == **dep)
                .map(|other| other.state != PlanState::Accepted)
                .unwrap_or(true)
        })
        .map(String::as_str)
        .collect();
    (!waiting.is_empty()).then(|| format!("task '{task_id}' waits for {}", waiting.join(", ")))
}

/// Joins two JSON objects, the second winning on shared keys. Journal events are built this way.
fn merged(mut base: Value, extra: Value) -> Value {
    if let (Some(target), Value::Object(fields)) = (base.as_object_mut(), extra) {
        target.extend(fields);
    }
    base
}

/// What one call asks admission for.
struct Admission<'a> {
    /// The node the call goes to.
    target: &'a str,
    stream_name: &'a str,
    task_id: Option<&'a str>,
    label: &'a str,
    max_retries: u32,
    /// The instance that made the call, and the worktree it works in.
    caller_instance: &'a str,
    caller_worktree: &'a Path,
    /// Whether the target shares the caller's worktree, and whether it writes there.
    shared: bool,
    writes: bool,
}

/// An admitted call: its attempt number, the instance number reserved for it, and its place in
/// `active`, which is released when the call ends.
struct Admitted {
    attempt: u32,
    instance_no: u32,
    registration: ActiveRegistration,
}

/// A delegated agent that is running now, and what it may do to the worktree it shares.
#[derive(Debug)]
pub struct ActiveInstance {
    pub worktree: PathBuf,
    pub writes: bool,
    /// The instance that delegated to this one. It is blocked until this one reports.
    pub parent: String,
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
    /// The repository's gates, read once when the run starts.
    pub gates: Vec<Gate>,
    instances: DashMap<String, Arc<Instance>>,
    tasks: DashMap<String, TaskStream>,
    /// The plan the root wrote. Empty until it plans, and then named tasks must come from it.
    plan: Mutex<Vec<PlannedTask>>,
    /// Delegated instances running right now, and the worktree each one works in.
    active: DashMap<String, ActiveInstance>,
    /// The run's integration worktree, created when the first isolated agent needs it. Accepted
    /// isolated work is merged here, and the next isolated agent starts from its tip.
    integration: tokio::sync::Mutex<Option<PathBuf>>,
    /// Serializes merges into the integration worktree.
    merge_lock: tokio::sync::Mutex<()>,
    /// Held while an attempt is admitted: the worktree check, the budget and the reservation of a
    /// place in `active` happen as one step, so two calls cannot both claim the same worktree.
    claims: Mutex<()>,
    instance_counters: DashMap<String, u32>,
    /// Instances of each node that are running right now. A node can fan out into several.
    running: DashMap<String, u32>,
    approvals: DashMap<String, Vec<oneshot::Sender<bool>>>,
}

impl RunContext {
    #[allow(clippy::too_many_arguments)]
    fn new(
        run_id: String,
        repo_root: PathBuf,
        graph: OrchestrationGraph,
        goal: String,
        token: String,
        run_dir: PathBuf,
        root_node_id: String,
        gates: Vec<Gate>,
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
            gates,
            instances: DashMap::new(),
            tasks: DashMap::new(),
            plan: Mutex::new(Vec::new()),
            active: DashMap::new(),
            integration: tokio::sync::Mutex::new(None),
            merge_lock: tokio::sync::Mutex::new(()),
            claims: Mutex::new(()),
            instance_counters: DashMap::new(),
            running: DashMap::new(),
            approvals: DashMap::new(),
        }
    }

    /// The running instance that a new agent in `worktree` would collide with, if any. Callers
    /// blocked on the new agent are not collisions: they are waiting, not writing. Two agents may
    /// share a worktree only when neither of them writes.
    fn worktree_conflict(&self, worktree: &Path, caller: &str, writes: bool) -> Option<String> {
        // The caller and everything above it are blocked on this agent, so they cannot be writing.
        let mut waiting = vec![caller.to_string()];
        let mut cursor = caller.to_string();
        while let Some(parent) = self.active.get(&cursor).map(|entry| entry.parent.clone()) {
            waiting.push(parent.clone());
            cursor = parent;
        }
        self.active
            .iter()
            .find(|entry| entry.worktree == worktree && (writes || entry.writes) && !waiting.contains(entry.key()))
            .map(|entry| entry.key().clone())
    }

    /// Admits one call, or refuses it, as a single step. The shared worktree is checked, the plan and
    /// the task budget are charged, and the new instance takes its place in `active`. Doing these
    /// together means two parallel calls can never both pass the worktree check.
    fn admit(self: &Arc<Self>, request: Admission<'_>) -> AppResult<Admitted> {
        let _claim = lock(&self.claims);
        if request.shared {
            if let Some(other) = self.worktree_conflict(request.caller_worktree, request.caller_instance, request.writes) {
                crate::error::bail!(
                    "the shared worktree is busy: '{other}' is working in it. Wait for it to finish, or run this agent in an isolated worktree."
                );
            }
        }
        let attempt = self.admit_attempt(request.stream_name, request.task_id, request.label, request.max_retries)?;
        let instance_no = self.next_instance_no(request.target);
        let instance_id = format!("{}.{instance_no}", request.target);
        // An isolated agent has a worktree of its own, which does not exist yet. Its placeholder
        // path matches no real worktree, so it never collides with anything.
        let worktree = if request.shared {
            request.caller_worktree.to_path_buf()
        } else {
            PathBuf::new()
        };
        let registration = ActiveRegistration::new(
            self.clone(),
            instance_id,
            worktree,
            request.writes,
            request.caller_instance.to_string(),
        );
        Ok(Admitted {
            attempt,
            instance_no,
            registration,
        })
    }

    /// Admits one attempt on a task, or refuses it. A run with a plan takes named tasks only, and a
    /// named task must be ready under that plan. Then the task's budget is charged. A refusal
    /// charges nothing, and the error says why, so the caller can change its plan or its approach.
    fn admit_attempt(&self, stream_name: &str, task_id: Option<&str>, label: &str, max_retries: u32) -> AppResult<u32> {
        {
            let plan = lock(&self.plan);
            if !plan.is_empty() {
                match task_id {
                    None => crate::error::bail!(
                        "this run has a plan, so name the task with a task_id from the plan"
                    ),
                    Some(id) => {
                        if let Some(reason) = plan_blocker(&plan, id) {
                            crate::error::bail!("{reason}");
                        }
                    }
                }
            }
        }
        let attempt = {
            let mut stream = self
                .tasks
                .entry(stream_name.to_string())
                .or_insert_with(|| TaskStream::new(task_id.is_some()));
            begin_attempt(&mut stream, max_retries, label)?
        };
        if let Some(id) = task_id {
            self.set_plan_state(id, PlanState::Running);
        }
        Ok(attempt)
    }

    /// Moves a planned task to `state`. Tasks that are not in the plan are unaffected.
    fn set_plan_state(&self, task_id: &str, state: PlanState) {
        let mut plan = lock(&self.plan);
        if let Some(task) = plan.iter_mut().find(|task| task.id == task_id) {
            task.state = state;
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

    /// Children are numbered from 1; the root is always `<node>.0`.
    fn next_instance_no(&self, node_id: &str) -> u32 {
        let mut count = self.instance_counters.entry(node_id.to_string()).or_insert(0);
        *count += 1;
        *count
    }

    fn instance_started(&self, node_id: &str) {
        *self.running.entry(node_id.to_string()).or_insert(0) += 1;
    }

    /// Records that one instance of `node_id` ended and returns how many are still running.
    fn instance_ended(&self, node_id: &str) -> u32 {
        let mut count = self.running.entry(node_id.to_string()).or_insert(1);
        *count = count.saturating_sub(1);
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
    pub async fn handle_message(self: &Arc<Self>, run_id: &str, instance_id: &str, message: Value) -> AppResult<Option<Value>> {
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
                    "serverInfo": { "name": "Condurence", "version": env!("CARGO_PKG_VERSION") }
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

    async fn call_tool(self: &Arc<Self>, run: &Arc<RunContext>, instance: &Arc<Instance>, params: Value) -> Result<Value, RpcError> {
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
        } else if name == compiler::PLAN_TOOL {
            self.plan_tasks(run, instance, &args)
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

    /// Records the plan. Only the root plans, and a plan can be rewritten until its tasks start.
    fn plan_tasks(&self, run: &RunContext, instance: &Instance, args: &Value) -> AppResult<String> {
        if !instance.is_root {
            crate::error::bail!("only the orchestrator plans the run; report your result instead");
        }
        let items = parse_plan(args)?;
        let next = {
            let mut plan = lock(&run.plan);
            let next = merge_plan(&plan, items)?;
            *plan = next.clone();
            next
        };
        self.log(
            Some(run),
            None,
            "plan",
            json!({
                "tasks": next.iter().map(|task| json!({
                    "id": task.id,
                    "title": task.title,
                    "dependsOn": task.depends_on,
                    "state": task.state,
                })).collect::<Vec<_>>(),
            }),
        );
        Ok(format!(
            "Plan recorded with {} tasks. Delegate each one by its id once the tasks it depends on are accepted.",
            next.len()
        ))
    }

    async fn dispatch(
        self: &Arc<Self>,
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
        // The attempt is charged to the task it belongs to, before anything is started or paused.
        let task_id = task_id_arg(args)?;
        let stream_name = stream_key(&edge.id, &parent.id, task_id.as_deref());
        let label = task_id
            .clone()
            .unwrap_or_else(|| format!("{} {target_id}", relation_label(relation)));
        let task_base = json!({
            "task": label,
            "taskId": task_id,
            "stream": stream_name,
            "relation": relation,
            "from": parent.node_id,
            "to": target_id,
            "maxRetries": edge.data.max_retries,
        });
        // A shared worktree runs one writer at a time, and a reader cannot share it with a writer.
        // An isolated agent has its own worktree, so it never collides with anything.
        let writes = target.data.access == AccessLevel::ReadWrite;
        let admission = run.admit(Admission {
            target: target_id,
            stream_name: &stream_name,
            task_id: task_id.as_deref(),
            label: &label,
            max_retries: edge.data.max_retries,
            caller_instance: &parent.id,
            caller_worktree: &parent.worktree,
            shared: target.data.worktree_mode == WorktreeMode::Shared,
            writes,
        });
        let admitted = match admission {
            Ok(admitted) => admitted,
            Err(error) => {
                self.log(
                    Some(run.as_ref()),
                    Some(target_id),
                    "task",
                    merged(task_base, json!({ "state": "refused", "reason": error.to_string() })),
                );
                return Err(error);
            }
        };
        let attempt = admitted.attempt;
        let instance_no = admitted.instance_no;
        // The reservation in `active` is held until this call ends, whatever happens to it.
        let _registration = admitted.registration;
        let remaining = edge.data.max_retries.saturating_sub(attempt);
        self.log(
            Some(run.as_ref()),
            Some(target_id),
            "task",
            merged(task_base.clone(), json!({ "state": "running", "attempt": attempt, "remaining": remaining })),
        );
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

        let instance_id = format!("{target_id}.{instance_no}");
        let worktree = self.provision_worktree(run, parent, &target, instance_no).await?;

        let path: Vec<String> = parent
            .chain
            .iter()
            .cloned()
            .chain(std::iter::once(parent.node_id.clone()))
            .collect();
        let chain = child_chain(path, target_id, relation);
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
        let invocation = self.build_invocation(run, &target, Mode::Headless, &system, &prompt, &instance_id, false)?;
        let result_file = invocation.result_file.clone();
        let session_files: Vec<PathBuf> = invocation.cleanup.iter().cloned().chain(result_file.clone()).collect();
        // A read-only agent is checked against the worktree as it was before it started.
        let read_only_snapshot = match target.data.access {
            AccessLevel::ReadOnly => Some(fingerprint(&worktree).await),
            AccessLevel::ReadWrite => None,
        };
        let session = self.spawn_session(run, &instance, invocation, &worktree)?;
        self.set_status(run, target_id, NodeStatus::Running, None, Some(session.id.clone()));
        run.instance_started(target_id);

        // If the caller goes away while this call is pending (its CLI exited or was killed), this
        // future is dropped. The guard then stops the child instead of leaving it running unseen.
        let mut guard = ChildGuard {
            router: self.clone(),
            run: run.clone(),
            node_id: target_id.to_string(),
            session_id: session.id.clone(),
            files: session_files,
            armed: true,
        };
        let outcome = self.await_outcome(run, &session, report_rx, result_file.as_deref()).await;
        guard.armed = false;
        // The outcome has been read from the session's files, and nothing will use them again.
        remove_session_files(&guard.files);
        let others_running = run.instance_ended(target_id) > 0;
        // The diff is taken before the gates run, so it shows the agent's work and nothing else.
        let stat = diff_stat(&worktree).await;
        // A read-only agent must leave the worktree as it found it, whatever it reported.
        let outcome = match read_only_snapshot {
            Some(before) => {
                if fingerprint(&worktree).await == before {
                    outcome
                } else {
                    read_only_violation(outcome, &stat)
                }
            }
            None => outcome,
        };
        // The child has exited, so the gates run against its finished work.
        let (outcome, gate_results) = check_gates(run, outcome, &worktree).await;
        for result in &gate_results {
            self.log(
                Some(run.as_ref()),
                Some(target_id),
                "gate",
                json!({
                    "gate": result.id,
                    "command": excerpt(&result.command, 300),
                    "passed": result.passed,
                    "exitCode": result.exit_code,
                    "timedOut": result.timed_out,
                    "outputTail": excerpt(&result.output_tail, 1000),
                    "instance": instance_id,
                }),
            );
        }
        // Work that passed its gates in an isolated worktree lands on the integration branch. A
        // conflict makes the attempt a failure, so the caller can retry it from the current tip.
        let (outcome, merge_state) = if outcome.status == "completed" && target.data.worktree_mode == WorktreeMode::Isolated {
            self.land_isolated_work(run, outcome, &worktree, &label).await
        } else {
            (outcome, Value::Null)
        };
        if !merge_state.is_null() {
            self.log(
                Some(run.as_ref()),
                Some(target_id),
                "merge",
                merged(json!({ "task": label, "instance": instance_id }), merge_state.clone()),
            );
        }
        // Settle the task. A pass closes a named task, and a failure that repeats the last one stalls it.
        let signature = attempt_signature(&outcome, &gate_results);
        if let Some(mut stream) = run.tasks.get_mut(&stream_name) {
            settle_attempt(&mut stream, outcome.status == "completed", signature.as_deref());
        }
        let task_state = run
            .tasks
            .get(&stream_name)
            .map(|stream| phase_label(stream.phase))
            .unwrap_or("open");
        if let Some(id) = task_id.as_deref() {
            let plan_state = match task_state {
                "accepted" => PlanState::Accepted,
                "stalled" => PlanState::Stalled,
                _ => PlanState::Pending,
            };
            run.set_plan_state(id, plan_state);
        }
        self.log(
            Some(run.as_ref()),
            Some(target_id),
            "task",
            merged(
                task_base,
                json!({
                    "state": task_state,
                    "attempt": attempt,
                    "remaining": remaining,
                    "status": outcome.status,
                    "signature": signature,
                }),
            ),
        );
        let gate_summary: Vec<Value> = gate_results
            .iter()
            .map(|result| json!({ "id": result.id, "passed": result.passed, "exitCode": result.exit_code, "timedOut": result.timed_out }))
            .collect();
        let status = if matches!(outcome.status.as_str(), "failed" | "blocked") {
            NodeStatus::Error
        } else {
            NodeStatus::Idle
        };
        // The card describes the node. While another instance of it is still working it stays on
        // running, and the last one to finish settles it.
        if !others_running {
            self.set_status(run, target_id, status, Some(excerpt(&outcome.headline, 200)), Some(session.id.clone()));
        }
        // What was asked and what came back, journaled with the run so the handoff can be read
        // again after the run ends.
        self.log(
            Some(run.as_ref()),
            Some(target_id),
            "handoff",
            json!({
                "relation": relation,
                "from": parent.node_id,
                "to": target_id,
                "instance": instance_id,
                "task": label,
                "attempt": attempt,
                "remaining": remaining,
                "taskState": task_state,
                "prompt": excerpt(&prompt, JOURNAL_TEXT_CHARS),
                "status": outcome.status,
                "exitCode": outcome.exit_code,
                "summary": excerpt(&outcome.summary, JOURNAL_TEXT_CHARS),
                "gates": gate_summary,
                "report": outcome.detail,
                "diffStat": stat,
            }),
        );

        Ok(json!({
            "node": target_id,
            "instance": instance_id,
            "task": label,
            "attempt": attempt,
            "remaining": remaining,
            "taskState": task_state,
            "status": outcome.status,
            "exitCode": outcome.exit_code,
            "summary": outcome.summary,
            "gates": gate_summary,
            "commandsRun": outcome.detail.commands_run,
            "openQuestions": outcome.detail.open_questions,
            "merge": merge_state,
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
                // Start from the integration tip, so work accepted earlier in the run is visible.
                let integration = self.integration_worktree(run).await?;
                let head = cmd::run(&integration, &["rev-parse", "HEAD"])
                    .await
                    .ok()
                    .map(|sha| sha.trim().to_string());
                let info = worktree::create_worktree(&self.locks, &run.repo_root, &branch, &folder, head.as_deref()).await?;
                Ok(PathBuf::from(info.path))
            }
        }
    }

    /// The run's integration worktree, created on first use from the repository's HEAD.
    async fn integration_worktree(&self, run: &RunContext) -> AppResult<PathBuf> {
        let mut slot = run.integration.lock().await;
        if let Some(path) = slot.as_ref() {
            return Ok(path.clone());
        }
        let short = run.short_id();
        let folder = format!("{short}-integration");
        let branch = format!("oc/{short}/integration");
        let head = cmd::run(&run.repo_root, &["rev-parse", "HEAD"])
            .await
            .ok()
            .map(|sha| sha.trim().to_string());
        let info = worktree::create_worktree(&self.locks, &run.repo_root, &branch, &folder, head.as_deref()).await?;
        let path = PathBuf::from(info.path);
        *slot = Some(path.clone());
        Ok(path)
    }

    /// Turns a merge into the attempt's outcome: a merged attempt keeps its result, and a conflict or
    /// a failed merge makes it a failure the caller can read and retry.
    async fn land_isolated_work(&self, run: &RunContext, outcome: Outcome, worktree: &Path, label: &str) -> (Outcome, Value) {
        match self.merge_into_integration(run, worktree, label).await {
            Ok(MergeOutcome::Merged { branch, commit }) => (
                outcome,
                json!({
                    "state": "merged",
                    "branch": branch,
                    "commit": commit,
                    "into": format!("oc/{}/integration", run.short_id()),
                }),
            ),
            Ok(MergeOutcome::NothingToMerge) => (outcome, json!({ "state": "nothing-to-merge" })),
            Ok(MergeOutcome::Conflict { branch, files }) => {
                let places = if files.is_empty() {
                    "files that could not be read".to_string()
                } else {
                    files.join(", ")
                };
                let failed = Outcome {
                    status: "failed".into(),
                    summary: format!(
                        "{}\n\nThe work conflicts with the integration branch in {places}, so it was not merged. Retry the task under the same task_id: the next attempt starts from the current integration branch.",
                        outcome.summary
                    ),
                    headline: "Merge conflict with the integration branch".into(),
                    detail: ReportDetail {
                        failure_signature: Some(format!("merge conflict in {places}")),
                        ..outcome.detail.clone()
                    },
                    ..outcome
                };
                (failed, json!({ "state": "conflict", "branch": branch, "files": files }))
            }
            Err(error) => {
                let reason = error.to_string();
                let failed = Outcome {
                    status: "failed".into(),
                    summary: format!("{}\n\nThe work could not be merged: {reason}", outcome.summary),
                    headline: "Merge failed".into(),
                    ..outcome
                };
                (failed, json!({ "state": "error", "reason": reason }))
            }
        }
    }

    /// Lands an accepted isolated agent's work on the integration branch. The agent's changes are
    /// committed on its branch, then merged with a merge commit, one merge at a time. A conflict is
    /// aborted, so the integration branch is never left half-merged.
    async fn merge_into_integration(&self, run: &RunContext, child: &Path, label: &str) -> AppResult<MergeOutcome> {
        let integration = self.integration_worktree(run).await?;
        let dirty = cmd::run(child, &["status", "--porcelain"]).await?;
        if !dirty.trim().is_empty() {
            cmd::run(child, &["add", "-A"]).await?;
            let message = format!("{label}: agent work");
            cmd::run(child, &["commit", "-q", "-m", message.as_str()])
                .await
                .map_err(|error| AppError::msg(format!("could not commit the agent's work: {error}")))?;
        }
        let branch = cmd::run(child, &["rev-parse", "--abbrev-ref", "HEAD"]).await?.trim().to_string();
        let behind = cmd::run(&integration, &["rev-parse", "HEAD"]).await?.trim().to_string();
        let range = format!("{behind}..HEAD");
        let ahead = cmd::run(child, &["rev-list", "--count", range.as_str()]).await?;
        if ahead.trim() == "0" {
            return Ok(MergeOutcome::NothingToMerge);
        }

        let _serial = run.merge_lock.lock().await;
        let message = format!("Merge {branch} ({label})");
        let merge = cmd::raw(
            &integration,
            &["merge", "--no-ff", "--no-edit", "-m", message.as_str(), branch.as_str()],
        )
        .await?;
        if merge.status.success() {
            let commit = cmd::run(&integration, &["rev-parse", "HEAD"]).await?.trim().to_string();
            return Ok(MergeOutcome::Merged { branch, commit });
        }
        let files: Vec<String> = cmd::run(&integration, &["diff", "--name-only", "--diff-filter=U"])
            .await
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect();
        let _ = cmd::raw(&integration, &["merge", "--abort"]).await;
        Ok(MergeOutcome::Conflict { branch, files })
    }

    fn build_invocation(
        &self,
        run: &RunContext,
        node: &GraphNode,
        mode: Mode,
        system: &str,
        prompt: &str,
        instance_id: &str,
        plan: bool,
    ) -> AppResult<Invocation> {
        let bridge = BridgeAddress {
            exe: &self.bridge_exe,
            port: self.port,
            run_id: &run.run_id,
            instance_id,
            token: &run.token,
        };
        compiler::invocation_with_access(
            node.data.provider,
            mode,
            &node.data.model,
            &node.data.effort,
            system,
            prompt,
            &bridge,
            &run.run_dir,
            node.data.access,
            plan,
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
        *lock(&instance.session) = Some(session_id.clone());

        // Record who the process is, so a launch after a crash can tell whether this agent is still
        // running and stop it. The start time is what tells it from an unrelated process that
        // later gets the same pid.
        let db = self.db.clone();
        let mut exit_rx = session.subscribe_exit();
        let pid = session.pid;
        tauri::async_runtime::spawn(async move {
            if let Some(pid) = pid {
                let identity = tauri::async_runtime::spawn_blocking(move || crate::pty::process_group::process_identity(pid))
                    .await
                    .ok()
                    .flatten();
                if let Some(started) = identity {
                    if let Err(e) = db.record_pty_process(&session_id, pid, &started) {
                        eprintln!("failed to record a session's process: {e}");
                    }
                }
            }
            // Record how the session ended, so the run history shows its exit code and end time.
            let code = wait_for_exit(&mut exit_rx).await;
            if let Err(e) = db.finish_pty_session(&session_id, code, now_ms()) {
                eprintln!("failed to record the end of a terminal session: {e}");
            }
        });

        Ok(session)
    }

    /// Blocks until the child reports or exits. The caller's `tools/call` stays pending
    /// for exactly this long.
    async fn await_outcome(
        &self,
        run: &RunContext,
        session: &Arc<PtySession>,
        report_rx: oneshot::Receiver<Report>,
        result_file: Option<&Path>,
    ) -> Outcome {
        let mut exit_rx = session.subscribe_exit();
        let mut report_rx = report_rx;
        let mut report_open = true;
        let mut report: Option<Report> = None;

        loop {
            tokio::select! {
                _ = run.cancel.cancelled() => {
                    self.terminate(session).await;
                    return resolve_outcome(None, session.exit_code(), true, &session.history(), None);
                }
                received = &mut report_rx, if report_open => {
                    report_open = false;
                    if let Ok(found) = received {
                        report = Some(found);
                        break;
                    }
                }
                code = wait_for_exit(&mut exit_rx) => {
                    return resolve_outcome(report, Some(code), false, &session.history(), read_final_message(result_file));
                }
            }
        }

        // The child reported: give it a moment to exit cleanly, then tear it down.
        match tokio::time::timeout(REPORT_GRACE, wait_for_exit(&mut exit_rx)).await {
            Ok(code) => resolve_outcome(report, Some(code), false, &session.history(), read_final_message(result_file)),
            Err(_) => {
                self.terminate(session).await;
                resolve_outcome(report, session.exit_code(), false, &session.history(), read_final_message(result_file))
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

        // Read before anything is recorded, so a broken gates file refuses the run outright.
        let gates = gates::load(&repo_root)?;
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
            gates,
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

        // Registered before the root starts: its bridge can connect the moment it is up, and an
        // unknown run is refused with 401.
        self.runs.insert(run_id.clone(), run.clone());

        let system = compiler::instructions(&run.graph, &root, &run.repo_root, false);
        let prompt = format!("# Goal\n{goal}");
        let plan = compiler::starts_in_plan_mode(&run.graph, &root);
        let launched = self
            .build_invocation(&run, &root, Mode::Interactive, &system, &prompt, &root_instance.id, plan)
            .and_then(|invocation| self.spawn_session(&run, &root_instance, invocation, &repo_root));
        let session = match launched {
            Ok(session) => session,
            Err(e) => {
                // Nothing is running, so the run must not linger as live or leave its token files.
                self.runs.remove(&run_id);
                let _ = self.db.finish_run(&run_id, "failed", now_ms());
                let _ = std::fs::remove_dir_all(&run.run_dir);
                return Err(e);
            }
        };
        self.set_status(&run, &root_node_id, NodeStatus::Running, None, Some(session.id.clone()));

        // The root is an interactive TUI: when the user exits it, the node goes idle again.
        let router = self.clone();
        let mut exit_rx = session.subscribe_exit();
        tauri::async_runtime::spawn(async move {
            let code = wait_for_exit(&mut exit_rx).await;
            let status = if code == 0 { NodeStatus::Idle } else { NodeStatus::Error };
            router.set_status(&run, &root_node_id, status, None, None);
            // Nobody is left to read a pending handoff's result. Cancelling stops those children
            // even when the root's bridge outlives it, blocked on a request it cannot abandon.
            run.cancel.cancel();
            // The root exiting ends the run, so it leaves the active set: its token stops
            // authorizing requests and a later stop cannot overwrite the outcome. A run that was
            // stopped has already been removed, and its recorded status is left alone.
            if router.runs.remove(&run.run_id).is_some() {
                let _ = router.db.finish_run(&run.run_id, "finished", now_ms());
            }
            // The run's MCP configs carry its bearer token and nothing can use them any more.
            let _ = std::fs::remove_dir_all(&run.run_dir);
        });
        Ok(run_id)
    }

    pub async fn stop_run(&self, run_id: &str) -> AppResult<()> {
        let Some((_, run)) = self.runs.remove(run_id) else {
            // A run whose root already exited has left the active set and been recorded as over.
            // Stopping it again changes nothing, so the Stop button still works on it.
            return match self.db.run_status(run_id)? {
                Some(status) if status != "running" => Ok(()),
                _ => Err(AppError::msg(format!("run '{run_id}' is not active"))),
            };
        };
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

/// Stops a delegated child when the call that started it is dropped before the child finished.
struct ChildGuard {
    router: Arc<Router>,
    run: Arc<RunContext>,
    node_id: String,
    session_id: String,
    /// Files made for this session, removed with it.
    files: Vec<PathBuf>,
    armed: bool,
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        remove_session_files(&self.files);
        // Terminating waits out a grace period, so it must not run on an async worker.
        let pty = self.router.pty.clone();
        let session_id = self.session_id.clone();
        std::thread::spawn(move || pty.terminate(&session_id));
        if self.run.instance_ended(&self.node_id) > 0 {
            return;
        }
        self.router.set_status(
            &self.run,
            &self.node_id,
            NodeStatus::Idle,
            Some("Stopped: the agent that delegated this went away".into()),
            Some(self.session_id.clone()),
        );
    }
}

/// Final result of one delegated execution.
#[derive(Debug, PartialEq)]
pub struct Outcome {
    pub status: String,
    pub exit_code: Option<i32>,
    pub summary: String,
    /// One line for the canvas card. The agent's own first line when it reported; otherwise the
    /// most telling line of its terminal output, never the echoed prompt at the top of it.
    pub headline: String,
    /// The structured evidence the agent reported. Empty when the result came from the exit code.
    pub detail: ReportDetail,
}

/// Pure outcome resolution: an explicit report wins, otherwise the exit code decides.
/// Output is taken from the stream-json result line when present, else from the tail.
fn resolve_outcome(
    report: Option<Report>,
    exit_code: Option<i32>,
    cancelled: bool,
    history: &str,
    final_message: Option<String>,
) -> Outcome {
    if cancelled {
        return Outcome {
            status: "cancelled".into(),
            exit_code,
            summary: final_text(history),
            headline: "Cancelled".into(),
            detail: ReportDetail::default(),
        };
    }
    if let Some(report) = report {
        let headline = headline_of(&report.summary, false, false);
        return Outcome {
            status: report.status,
            exit_code,
            summary: report.summary,
            headline,
            detail: report.detail,
        };
    }
    let failed = exit_code != Some(0);
    // The CLI's own record of its final reply beats anything scraped from the terminal.
    if let Some(message) = final_message.map(|text| text.trim().to_string()).filter(|text| !text.is_empty()) {
        let headline = headline_of(&message, false, false);
        return Outcome {
            status: if failed { "failed" } else { "completed" }.into(),
            exit_code,
            summary: message,
            headline,
            detail: ReportDetail::default(),
        };
    }
    let clean = strip_ansi(history);
    // A structured result line is the agent's own answer. Without one, the output is a raw
    // terminal log whose first lines are the echoed prompt, so look at its end instead.
    let (summary, from_log) = match stream_result(&clean) {
        Some(text) => (text, false),
        None => (excerpt_tail(clean.trim(), SUMMARY_TAIL_CHARS), true),
    };
    let headline = headline_of(&summary, from_log, failed);
    Outcome {
        status: if failed { "failed" } else { "completed" }.into(),
        exit_code,
        summary,
        headline,
        detail: ReportDetail::default(),
    }
}

/// What a worktree looks like right now: its changes against the base, and the size and time of
/// each changed file. Equal fingerprints mean nothing changed between them.
async fn fingerprint(worktree: &Path) -> String {
    let base = if cmd::run(worktree, &["rev-parse", "--verify", "-q", "HEAD"]).await.is_ok() {
        "HEAD".to_string()
    } else {
        EMPTY_TREE.to_string()
    };
    let status = cmd::run(worktree, &["status", "--porcelain=v1", "-z", "--untracked-files=all"])
        .await
        .unwrap_or_default();
    let diff = cmd::run(worktree, &["diff", "--no-ext-diff", "--binary", base.as_str()])
        .await
        .unwrap_or_default();
    let mut out = format!("{status}\0{diff}\0");
    // Each porcelain entry is `XY path`. Editing a file that is already changed leaves the status
    // line as it was, so the file's size and time go into the fingerprint too.
    for entry in status.split('\0').filter(|entry| entry.len() > 3) {
        let path = &entry[3..];
        let meta = std::fs::metadata(worktree.join(path)).ok().map(|m| (m.len(), m.modified().ok()));
        out.push_str(&format!("{path}={meta:?}\0"));
    }
    out
}

/// A read-only agent changed its worktree. Whatever it reported, the attempt failed, and the same
/// violation twice stalls the task like any other repeated failure.
fn read_only_violation(outcome: Outcome, stat: &str) -> Outcome {
    let changes = if stat.is_empty() {
        "the worktree changed".to_string()
    } else {
        stat.to_string()
    };
    Outcome {
        status: "failed".into(),
        summary: format!(
            "{}\n\nThis agent is read-only, but its worktree changed while it ran:\n{changes}",
            outcome.summary
        ),
        headline: "Read-only agent changed files".into(),
        detail: ReportDetail {
            failure_signature: Some("access read-only violation".into()),
            ..outcome.detail.clone()
        },
        ..outcome
    }
}

/// Runs the repository's gates on a `completed` report, in the child's worktree. Anything else
/// has nothing to prove, so no gates run for it.
async fn check_gates(run: &RunContext, outcome: Outcome, worktree: &Path) -> (Outcome, Vec<GateResult>) {
    if outcome.status != "completed" || run.gates.is_empty() {
        return (outcome, Vec::new());
    }
    let results = gates::run_all(&run.gates, worktree, &run.run_dir, &run.cancel).await;
    (apply_gates(outcome, &results), results)
}

/// A `completed` report stands only if every gate passed. The first failing gate turns it into a
/// failure, and its output becomes the evidence the caller reads.
fn apply_gates(outcome: Outcome, results: &[GateResult]) -> Outcome {
    let Some(failed) = results.iter().find(|result| !result.passed) else {
        return outcome;
    };
    let how = if failed.timed_out {
        "timed out".to_string()
    } else {
        match failed.exit_code {
            Some(code) => format!("exited {code}"),
            None => "was stopped".to_string(),
        }
    };
    let summary = format!(
        "{}\n\nThe agent reported completion, but gate '{}' failed ({how}).\n{}",
        outcome.summary,
        failed.id,
        excerpt(&failed.output_tail, 1000)
    );
    Outcome {
        status: "failed".into(),
        summary,
        headline: format!("Gate '{}' failed", failed.id),
        ..outcome
    }
}

/// What a failed attempt has in common with the same failure next time, for stall detection. The
/// first failing gate's signature wins, then the agent's own, then its status and headline.
fn attempt_signature(outcome: &Outcome, results: &[GateResult]) -> Option<String> {
    if let Some(failed) = results.iter().find(|result| !result.passed) {
        return Some(gates::signature(failed));
    }
    if outcome.status == "completed" {
        return None;
    }
    Some(match outcome.detail.failure_signature.as_deref() {
        Some(signature) => format!("report:{signature}"),
        None => format!("{}:{}", outcome.status, gates::normalize(&outcome.headline)),
    })
}

fn phase_label(phase: TaskPhase) -> &'static str {
    match phase {
        TaskPhase::Open => "open",
        TaskPhase::Accepted => "accepted",
        TaskPhase::Stalled => "stalled",
    }
}

/// One card-sized line from `text`. For raw terminal logs of a failed run it prefers the last
/// line that mentions an error, so "402 Payment Required" shows instead of the prompt echo.
fn headline_of(text: &str, from_log: bool, failed: bool) -> String {
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
    let Some(first) = lines.first() else {
        return String::new();
    };
    if !from_log {
        return (*first).to_string();
    }
    if failed {
        let telling = lines.iter().rev().find(|line| {
            let lower = line.to_lowercase();
            ["error", "failed", "denied", "refused", "unauthorized"].iter().any(|word| lower.contains(word))
        });
        if let Some(line) = telling {
            return (*line).to_string();
        }
    }
    lines.last().map(|line| (*line).to_string()).unwrap_or_default()
}

/// Enforces acyclic delegation and escalation. Verification edges may loop (the Senior Dev <->
/// Debugger pattern); they are bounded by the edge attempt budget instead. Escalation may climb
/// back to an ancestor, which is what it is for (`escalates_to` into the root restarts the root),
/// so only escalating to oneself is a cycle there.
pub fn check_hop(chain: &[String], from: &str, target: &str, relation: RelationType) -> AppResult<()> {
    if relation == RelationType::Verifies {
        return Ok(());
    }
    let climbs = relation == RelationType::EscalatesTo;
    if from == target || (!climbs && chain.iter().any(|node| node == target)) {
        let path = chain.iter().map(String::as_str).chain([from, target]).collect::<Vec<_>>().join(" -> ");
        crate::error::bail!("cycle detected: {path}");
    }
    Ok(())
}

/// The ancestry a new instance of `target` runs under, given the path that led to the caller
/// (`path` ends with the caller). An escalation into an ancestor restarts that ancestor where it
/// stood, so it keeps only the nodes above it and may delegate down the same path again. Every
/// other hop appends the caller.
fn child_chain(path: Vec<String>, target: &str, relation: RelationType) -> Vec<String> {
    match path.iter().position(|node| node == target) {
        Some(index) if relation == RelationType::EscalatesTo => path[..index].to_vec(),
        _ => path,
    }
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

/// Deletes the files and directories made for one session. Symbolic links inside a directory are
/// removed without following them, so a link to the user's own configuration stays intact.
fn remove_session_files(files: &[PathBuf]) {
    for path in files {
        let removed = match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(path),
            Ok(_) => std::fs::remove_file(path),
            Err(_) => continue,
        };
        if let Err(e) = removed {
            eprintln!("failed to remove session file {}: {e}", path.display());
        }
    }
}

/// The final reply a CLI wrote to its result file, if it wrote one.
fn read_final_message(file: Option<&Path>) -> Option<String> {
    std::fs::read_to_string(file?).ok()
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
    stream_result(&clean).unwrap_or_else(|| excerpt_tail(clean.trim(), SUMMARY_TAIL_CHARS))
}

/// The `result` of the last Claude stream-json `{"type":"result"}` line, if there is one.
fn stream_result(clean: &str) -> Option<String> {
    for line in clean.lines().rev() {
        let trimmed = line.trim();
        if !trimmed.starts_with('{') {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
            if value.get("type").and_then(Value::as_str) == Some("result") {
                if let Some(text) = value.get("result").and_then(Value::as_str) {
                    return Some(text.to_string());
                }
            }
        }
    }
    None
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
            detail: ReportDetail::default(),
        };
        let outcome = resolve_outcome(Some(report), Some(0), false, "", None);
        assert_eq!(outcome.status, "blocked");
        assert_eq!(outcome.summary, "needs API key");
    }

    #[test]
    fn exit_code_decides_without_report() {
        assert_eq!(resolve_outcome(None, Some(0), false, "done", None).status, "completed");
        assert_eq!(resolve_outcome(None, Some(1), false, "boom", None).status, "failed");
        assert_eq!(resolve_outcome(None, None, true, "", None).status, "cancelled");
    }

    #[cfg(unix)]
    #[test]
    fn session_files_are_removed_without_touching_what_their_links_point_to() {
        let dir = tempfile::tempdir().unwrap();
        let real_home = dir.path().join("real-gemini");
        std::fs::create_dir_all(&real_home).unwrap();
        std::fs::write(real_home.join("oauth_creds.json"), "secret").unwrap();

        // An ephemeral home links the user's real entries, as `prepare_gemini_home` does.
        let ephemeral = dir.path().join("n.1.gemini-home");
        std::fs::create_dir_all(&ephemeral).unwrap();
        std::os::unix::fs::symlink(real_home.join("oauth_creds.json"), ephemeral.join("oauth_creds.json")).unwrap();
        std::os::unix::fs::symlink(&real_home, ephemeral.join("linked-dir")).unwrap();
        let config = dir.path().join("n.1.claude-mcp.json");
        std::fs::write(&config, "{}").unwrap();

        remove_session_files(&[ephemeral.clone(), config.clone(), dir.path().join("never-existed")]);

        assert!(!ephemeral.exists() && !config.exists());
        assert_eq!(std::fs::read_to_string(real_home.join("oauth_creds.json")).unwrap(), "secret");
    }

    #[test]
    fn a_cli_result_file_beats_the_terminal_tail() {
        // Codex ends its log with a token counter, which is not the answer.
        let log = "user\r\nthe prompt echo\r\nHello!\r\ntokens used\r\n4,028\r\n";
        let outcome = resolve_outcome(None, Some(0), false, log, Some("Hello from codex\n".into()));
        assert_eq!(outcome.status, "completed");
        assert_eq!(outcome.summary, "Hello from codex");
        assert_eq!(outcome.headline, "Hello from codex");

        // An empty file says nothing, so the log is used, and an explicit report still wins.
        assert_eq!(resolve_outcome(None, Some(0), false, "tail text", Some("  \n".into())).summary, "tail text");
        let report = Report { status: "blocked".into(), summary: "needs input".into(), detail: ReportDetail::default() };
        let reported = resolve_outcome(Some(report), Some(0), false, "", Some("ignored".into()));
        assert_eq!((reported.status.as_str(), reported.summary.as_str()), ("blocked", "needs input"));
    }

    #[test]
    fn failed_log_headline_shows_the_error_not_the_echoed_prompt() {
        let log = "user\r\n## Completion\r\nWhen your task is finished call report_task_result\r\n\r\nERROR: Reconnecting... 5/5\r\nERROR: unexpected status 402 Payment Required\r\n";
        let outcome = resolve_outcome(None, Some(1), false, log, None);
        assert_eq!(outcome.status, "failed");
        assert_eq!(outcome.headline, "ERROR: unexpected status 402 Payment Required");
    }

    #[test]
    fn successful_log_headline_is_the_last_line_and_reports_use_their_first_line() {
        let outcome = resolve_outcome(None, Some(0), false, "user\r\nthe prompt echo\r\nHello from codex\r\n", None);
        assert_eq!(outcome.headline, "Hello from codex");

        let report = Report { status: "completed".into(), summary: "Added greeting.\nSecond line.".into(), detail: ReportDetail::default() };
        assert_eq!(resolve_outcome(Some(report), Some(0), false, "", None).headline, "Added greeting.");

        let json = "{\"type\":\"result\",\"result\":\"Hello!\\nBye\"}\r\n";
        assert_eq!(resolve_outcome(None, Some(0), false, json, None).headline, "Hello!");
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
    fn escalation_climbs_to_an_ancestor_but_not_to_itself() {
        let chain = vec!["ceo".to_string(), "senior-dev".to_string()];
        assert!(check_hop(&chain, "debugger", "ceo", RelationType::EscalatesTo).is_ok());
        assert!(check_hop(&chain, "debugger", "senior-dev", RelationType::EscalatesTo).is_ok());
        assert!(check_hop(&chain, "debugger", "debugger", RelationType::EscalatesTo).is_err());
        assert!(check_hop(&chain, "debugger", "ceo", RelationType::DelegatesTo).is_err());
    }

    #[test]
    fn escalation_restarts_an_ancestor_with_only_the_nodes_above_it() {
        let path = vec!["ceo".to_string(), "senior-dev".to_string(), "debugger".to_string()];
        assert_eq!(child_chain(path.clone(), "ceo", RelationType::EscalatesTo), Vec::<String>::new());
        assert_eq!(child_chain(path.clone(), "senior-dev", RelationType::EscalatesTo), vec!["ceo".to_string()]);
        assert_eq!(child_chain(path.clone(), "qa", RelationType::DelegatesTo), path);
        assert_eq!(child_chain(path.clone(), "senior-dev", RelationType::Verifies), path);
    }

    #[test]
    fn delegate_prompt_requires_task() {
        assert!(compose_prompt(RelationType::DelegatesTo, "ceo", &json!({})).is_err());
        let prompt = compose_prompt(RelationType::DelegatesTo, "ceo", &json!({"task": "add login", "target_files": ["src/a.ts"]})).unwrap();
        assert!(prompt.contains("add login"));
        assert!(prompt.contains("src/a.ts"));
    }

    fn plan_args(value: Value) -> Value {
        json!({ "tasks": value })
    }

    #[test]
    fn a_plan_is_checked_before_it_is_recorded() {
        let ok = parse_plan(&plan_args(json!([
            { "id": "schema", "title": "Add the schema" },
            { "id": "api", "title": "Expose it", "depends_on": ["schema"] },
        ])))
        .unwrap();
        assert_eq!(ok.len(), 2);
        assert_eq!(ok[1].2, vec!["schema".to_string()]);

        assert!(parse_plan(&plan_args(json!([]))).is_err(), "an empty plan says nothing");
        assert!(parse_plan(&plan_args(json!([{ "id": "bad id", "title": "x" }]))).is_err());
        assert!(parse_plan(&plan_args(json!([{ "id": "a", "title": "x" }, { "id": "a", "title": "y" }]))).is_err());
        assert!(parse_plan(&plan_args(json!([{ "id": "a", "title": "  " }]))).is_err());
        assert!(parse_plan(&plan_args(json!([{ "id": "a", "title": "x", "depends_on": ["ghost"] }]))).is_err());
        assert!(parse_plan(&plan_args(json!([{ "id": "a", "title": "x", "depends_on": ["a"] }]))).is_err());
        let cycle = parse_plan(&plan_args(json!([
            { "id": "a", "title": "x", "depends_on": ["b"] },
            { "id": "b", "title": "y", "depends_on": ["a"] },
        ])));
        assert!(cycle.unwrap_err().to_string().contains("cycle"));
    }

    #[test]
    fn a_replan_keeps_what_has_started() {
        let old = vec![
            PlannedTask { id: "a".into(), title: "A".into(), depends_on: vec![], state: PlanState::Accepted },
            PlannedTask { id: "b".into(), title: "B".into(), depends_on: vec!["a".into()], state: PlanState::Pending },
        ];
        // Dropping an accepted task is a rewrite of history, so it is refused.
        assert!(merge_plan(&old, vec![("b".into(), "B".into(), vec![])]).is_err());
        // Keeping it keeps its state, and an unstarted task can be rewritten freely.
        let merged = merge_plan(
            &old,
            vec![
                ("a".into(), "A".into(), vec![]),
                ("b".into(), "B, revised".into(), vec!["a".into()]),
                ("c".into(), "C".into(), vec!["b".into()]),
            ],
        )
        .unwrap();
        assert_eq!(merged[0].state, PlanState::Accepted);
        assert_eq!(merged[1].title, "B, revised");
        assert_eq!(merged[1].state, PlanState::Pending);
        assert_eq!(merged[2].state, PlanState::Pending);
    }

    #[test]
    fn a_task_waits_until_its_dependencies_are_accepted() {
        let plan = vec![
            PlannedTask { id: "a".into(), title: "A".into(), depends_on: vec![], state: PlanState::Pending },
            PlannedTask { id: "b".into(), title: "B".into(), depends_on: vec!["a".into()], state: PlanState::Pending },
        ];
        assert_eq!(plan_blocker(&plan, "a"), None);
        assert_eq!(plan_blocker(&plan, "b").as_deref(), Some("task 'b' waits for a"));
        assert!(plan_blocker(&plan, "ghost").unwrap().contains("not in the plan"));
        let mut ready = plan.clone();
        ready[0].state = PlanState::Accepted;
        assert_eq!(plan_blocker(&ready, "b"), None);
    }

    #[test]
    fn a_run_with_a_plan_admits_only_ready_planned_tasks() {
        let run = RunContext::new(
            "run-1".into(),
            PathBuf::from("/repo"),
            crate::graph::default_graph(),
            "goal".into(),
            "token".into(),
            PathBuf::from("/tmp/run-1"),
            "ceo".into(),
            Vec::new(),
        );
        // No plan yet: unnamed calls are admitted as they always were.
        assert_eq!(run.admit_attempt("e1@ceo.0", None, "delegates to senior-dev", 3).unwrap(), 1);

        run.plan.lock().unwrap().extend(
            merge_plan(
                &[],
                vec![("a".into(), "A".into(), vec![]), ("b".into(), "B".into(), vec!["a".into()])],
            )
            .unwrap(),
        );
        let unnamed = run.admit_attempt("e1@ceo.0", None, "x", 3).unwrap_err().to_string();
        assert!(unnamed.contains("task_id"), "unexpected refusal: {unnamed}");
        let waiting = run.admit_attempt("e1#b", Some("b"), "b", 3).unwrap_err().to_string();
        assert!(waiting.contains("waits for a"), "unexpected refusal: {waiting}");

        assert_eq!(run.admit_attempt("e1#a", Some("a"), "a", 3).unwrap(), 1);
        assert_eq!(run.plan.lock().unwrap()[0].state, PlanState::Running);
    }

    fn test_run() -> Arc<RunContext> {
        Arc::new(RunContext::new(
            "run-worktrees".into(),
            PathBuf::from("/repo"),
            crate::graph::default_graph(),
            "goal".into(),
            "token".into(),
            PathBuf::from("/tmp/run-worktrees"),
            "ceo".into(),
            Vec::new(),
        ))
    }

    #[test]
    fn a_shared_worktree_takes_one_writer_at_a_time() {
        let run = test_run();
        let repo = PathBuf::from("/repo");
        run.active.insert(
            "senior-dev.1".into(),
            ActiveInstance { worktree: repo.clone(), writes: true, parent: "ceo.0".into() },
        );
        // A second writer from the same orchestrator collides with the first.
        assert_eq!(run.worktree_conflict(&repo, "ceo.0", true).as_deref(), Some("senior-dev.1"));
        // A reader collides with a writer too: its view would change under it.
        assert!(run.worktree_conflict(&repo, "ceo.0", false).is_some());
        // The writer's own child is not a collision, because the writer is blocked on it.
        assert_eq!(run.worktree_conflict(&repo, "senior-dev.1", true), None);
        // Another worktree is free.
        assert_eq!(run.worktree_conflict(Path::new("/elsewhere"), "ceo.0", true), None);
    }

    #[test]
    fn readers_may_share_a_worktree() {
        let run = test_run();
        let shared = PathBuf::from("/review");
        run.active.insert(
            "debugger.1".into(),
            ActiveInstance { worktree: shared.clone(), writes: false, parent: "ceo.0".into() },
        );
        assert_eq!(run.worktree_conflict(&shared, "ceo.0", false), None);
    }

    #[test]
    fn admission_reserves_a_shared_worktree_before_the_second_call_arrives() {
        let run = test_run();
        let repo = PathBuf::from("/repo");
        let request = || Admission {
            target: "senior-dev",
            stream_name: "e1@ceo.0",
            task_id: None,
            label: "delegates to senior-dev",
            max_retries: 3,
            caller_instance: "ceo.0",
            caller_worktree: &repo,
            shared: true,
            writes: true,
        };
        let first = run.admit(request()).unwrap();
        assert_eq!(first.instance_no, 1);
        // The first call's reservation is still held, so the second is refused without running.
        let refused = run.admit(request()).err().expect("the second writer must be refused");
        assert!(refused.to_string().contains("busy"), "unexpected refusal: {refused}");
        // Once the first call ends, the worktree is free again.
        drop(first);
        assert!(run.admit(request()).is_ok());
    }

    #[test]
    fn isolated_agents_never_collide_with_a_shared_worktree() {
        let run = test_run();
        let repo = PathBuf::from("/repo");
        let writer = run
            .admit(Admission {
                target: "senior-dev",
                stream_name: "e1@ceo.0",
                task_id: None,
                label: "delegates to senior-dev",
                max_retries: 3,
                caller_instance: "ceo.0",
                caller_worktree: &repo,
                shared: true,
                writes: true,
            })
            .unwrap();
        let isolated = run.admit(Admission {
            target: "debugger",
            stream_name: "e2@ceo.0",
            task_id: None,
            label: "verifies with debugger",
            max_retries: 3,
            caller_instance: "ceo.0",
            caller_worktree: &repo,
            shared: false,
            writes: true,
        });
        assert!(isolated.is_ok(), "an isolated agent has its own worktree");
        drop(writer);
    }

    #[tokio::test]
    async fn a_fingerprint_changes_when_anything_in_the_worktree_changes() {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .current_dir(dir.path())
                .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
                .args(args)
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?} failed");
        };
        git(&["init", "-q", "-b", "main"]);
        std::fs::write(dir.path().join("README.md"), "# demo\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "init"]);

        let base = fingerprint(dir.path()).await;
        assert_eq!(base, fingerprint(dir.path()).await, "an untouched worktree fingerprints the same way");
        std::fs::write(dir.path().join("new.txt"), "hi").unwrap();
        let untracked = fingerprint(dir.path()).await;
        assert_ne!(base, untracked, "a new file is a change");
        std::fs::write(dir.path().join("new.txt"), "hello there").unwrap();
        assert_ne!(untracked, fingerprint(dir.path()).await, "a further edit to a changed file is a change");
    }

    fn gate_result(id: &str, passed: bool, output: &str) -> GateResult {
        GateResult {
            id: id.into(),
            command: format!("run {id}"),
            passed,
            exit_code: Some(if passed { 0 } else { 1 }),
            timed_out: false,
            output_tail: output.into(),
        }
    }

    fn completed(summary: &str) -> Outcome {
        resolve_outcome(
            Some(Report {
                status: "completed".into(),
                summary: summary.into(),
                detail: ReportDetail::default(),
            }),
            Some(0),
            false,
            "",
            None,
        )
    }

    #[test]
    fn an_unnamed_stream_is_bounded_by_maxretries() {
        let mut stream = TaskStream::new(false);
        assert_eq!(begin_attempt(&mut stream, 2, "verifies with debugger").unwrap(), 1);
        settle_attempt(&mut stream, false, Some("a"));
        assert_eq!(begin_attempt(&mut stream, 2, "verifies with debugger").unwrap(), 2);
        settle_attempt(&mut stream, false, Some("b"));
        let refused = begin_attempt(&mut stream, 2, "verifies with debugger").unwrap_err().to_string();
        assert!(refused.contains("maxRetries"), "unexpected refusal: {refused}");
        assert_eq!(stream.attempts, 2, "a refused attempt is not counted");
    }

    #[test]
    fn a_named_task_has_its_own_budget() {
        let edge = stream_key("e1", "ceo.0", Some("login"));
        let other = stream_key("e1", "ceo.0", Some("signup"));
        assert_ne!(edge, other);
        // Work on one task never draws on another task's budget.
        let mut login = TaskStream::new(true);
        let mut signup = TaskStream::new(true);
        begin_attempt(&mut login, 1, "login").unwrap();
        assert!(begin_attempt(&mut login, 1, "login").is_err());
        assert_eq!(begin_attempt(&mut signup, 1, "signup").unwrap(), 1);
    }

    #[test]
    fn an_unnamed_stream_is_per_caller_and_edge() {
        assert_eq!(stream_key("e1", "senior-dev.1", None), "e1@senior-dev.1");
        assert_ne!(stream_key("e1", "senior-dev.1", None), stream_key("e1", "senior-dev.2", None));
        assert_ne!(stream_key("e1", "senior-dev.1", None), stream_key("e2", "senior-dev.1", None));
    }

    #[test]
    fn a_passed_named_task_cannot_be_retried() {
        let mut stream = TaskStream::new(true);
        begin_attempt(&mut stream, 3, "login").unwrap();
        settle_attempt(&mut stream, true, None);
        assert_eq!(stream.phase, TaskPhase::Accepted);
        let refused = begin_attempt(&mut stream, 3, "login").unwrap_err().to_string();
        assert!(refused.contains("already accepted"), "unexpected refusal: {refused}");
    }

    #[test]
    fn a_passed_unnamed_stream_stays_open() {
        // Callers that do not name their tasks keep the behaviour they had before task ids.
        let mut stream = TaskStream::new(false);
        begin_attempt(&mut stream, 3, "delegates to senior-dev").unwrap();
        settle_attempt(&mut stream, true, None);
        assert_eq!(stream.phase, TaskPhase::Open);
        assert!(begin_attempt(&mut stream, 3, "delegates to senior-dev").is_ok());
    }

    #[test]
    fn the_same_failure_twice_stalls_the_task() {
        let mut stream = TaskStream::new(true);
        begin_attempt(&mut stream, 5, "login").unwrap();
        settle_attempt(&mut stream, false, Some("gate:test:exit1:assert"));
        assert_eq!(stream.phase, TaskPhase::Open, "one failure is not a stall");
        begin_attempt(&mut stream, 5, "login").unwrap();
        settle_attempt(&mut stream, false, Some("gate:test:exit1:assert"));
        assert_eq!(stream.phase, TaskPhase::Stalled);
        let refused = begin_attempt(&mut stream, 5, "login").unwrap_err().to_string();
        assert!(refused.contains("stalled"), "unexpected refusal: {refused}");
    }

    #[test]
    fn a_different_failure_does_not_stall_the_task() {
        let mut stream = TaskStream::new(true);
        begin_attempt(&mut stream, 5, "login").unwrap();
        settle_attempt(&mut stream, false, Some("gate:test:exit1:assert"));
        begin_attempt(&mut stream, 5, "login").unwrap();
        settle_attempt(&mut stream, false, Some("gate:build:exit2:syntax"));
        assert_eq!(stream.phase, TaskPhase::Open);
    }

    #[test]
    fn task_ids_are_short_and_plain() {
        let args = |value: Value| json!({ "task_id": value });
        assert_eq!(task_id_arg(&json!({})).unwrap(), None);
        assert_eq!(task_id_arg(&args(json!("  login-2 "))).unwrap(), Some("login-2".into()));
        assert_eq!(task_id_arg(&args(json!(""))).unwrap(), None);
        assert!(task_id_arg(&args(json!("has space"))).is_err());
        assert!(task_id_arg(&args(json!("a#b"))).is_err());
        assert!(task_id_arg(&args(json!("x".repeat(65)))).is_err());
        assert!(task_id_arg(&args(json!(7))).is_err());
    }

    #[test]
    fn a_report_is_checked_before_it_is_recorded() {
        let good = parse_report(&json!({
            "status": "completed",
            "summary": "done",
            "changed_files": ["src/a.ts"],
            "commands_run": [{ "command": "npm test", "exit_code": 0, "excerpt": "ok" }],
            "open_questions": ["none"],
            "failure_signature": "Gate test FAILED at 12:03",
        }))
        .unwrap();
        assert_eq!(good.status, "completed");
        assert_eq!(good.detail.changed_files, vec!["src/a.ts".to_string()]);
        assert_eq!(good.detail.commands_run[0]["exitCode"], json!(0));
        assert_eq!(good.detail.failure_signature.as_deref(), Some("gate test failed at ##:##"));

        assert!(parse_report(&json!({ "status": "maybe" })).is_err());
        assert!(parse_report(&json!({ "commands_run": [{ "exit_code": 0 }] })).is_err());
        assert!(parse_report(&json!({ "commands_run": [{ "command": "x", "exit_code": "zero" }] })).is_err());
        assert!(parse_report(&json!({ "changed_files": "a.ts" })).is_err());
    }

    #[test]
    fn a_failing_gate_overrides_a_completed_report() {
        let outcome = completed("All done, tests pass.");
        let results = vec![gate_result("test", false, "1 failing test")];
        let corrected = apply_gates(outcome, &results);
        assert_eq!(corrected.status, "failed");
        assert!(corrected.summary.starts_with("All done, tests pass."));
        assert!(corrected.summary.contains("gate 'test' failed"));
        assert!(corrected.summary.contains("1 failing test"));
        assert_eq!(corrected.headline, "Gate 'test' failed");
    }

    #[test]
    fn passing_gates_leave_a_completed_report_alone() {
        let outcome = completed("done");
        let results = vec![gate_result("test", true, "ok")];
        let kept = apply_gates(outcome, &results);
        assert_eq!(kept.status, "completed");
        assert_eq!(kept.summary, "done");
    }

    #[test]
    fn a_failed_attempt_is_signed_by_its_first_failing_gate_or_its_report() {
        let failed_gate = gate_result("test", false, "AssertionError at line 42");
        let outcome = completed("done");
        let signature = attempt_signature(&apply_gates(outcome, std::slice::from_ref(&failed_gate)), std::slice::from_ref(&failed_gate));
        assert_eq!(signature.as_deref(), Some(gates::signature(&failed_gate).as_str()));

        let report = Report {
            status: "blocked".into(),
            summary: "needs an API key".into(),
            detail: ReportDetail {
                failure_signature: Some("missing api key".into()),
                ..ReportDetail::default()
            },
        };
        let blocked = resolve_outcome(Some(report), Some(0), false, "", None);
        assert_eq!(attempt_signature(&blocked, &[]).as_deref(), Some("report:missing api key"));

        assert_eq!(attempt_signature(&completed("done"), &[]), None, "a pass has no failure to repeat");
    }

    #[test]
    fn constant_time_compare_matches_equality() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }
}
