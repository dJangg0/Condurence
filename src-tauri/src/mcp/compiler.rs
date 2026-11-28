//! Graph-to-MCP compilation: turns a node's outgoing edges into MCP tools, writes the
//! ephemeral instruction file, and builds the per-provider launch command.
//!
//! Everything written here lives under the OS temp directory (never inside the tracked
//! repository) and is removed when the run stops.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::error::AppResult;
use crate::graph::{AccessLevel, AgentProvider, GraphNode, OrchestrationGraph, RelationType, WorktreeMode};

/// MCP server key registered with every provider CLI.
pub const SERVER_KEY: &str = "Condurence";
/// Injected into delegated (headless) agents so they can hand back a structured result.
pub const REPORT_TOOL: &str = "report_task_result";
/// Given to the root orchestrator only: it writes the plan the run follows.
pub const PLAN_TOOL: &str = "plan_tasks";
/// Shared by every tool that can continue a task, so the agents see one explanation of retries.
const TASK_ID_DESCRIPTION: &str = "Optional. Use the same id when you retry the same task, so its retries share one maxRetries budget and stall detection. Use a new id for new work. A task that already passed its gates cannot be retried under its id.";
/// How long a Codex agent waits on one orchestrator tool call. A delegation blocks until the child
/// finishes, which routinely takes longer than Codex's 5 minute default, and Codex then abandons the
/// call while the child keeps running. Stopping the run is the way to end a stuck one sooner.
const CODEX_TOOL_TIMEOUT_SECS: u64 = 4 * 60 * 60;

/// The directory that holds one subdirectory per run.
pub fn run_dirs_root() -> PathBuf {
    std::env::temp_dir().join("Condurence")
}

pub fn run_dir(run_id: &str) -> PathBuf {
    run_dirs_root().join(run_id)
}

pub fn edge_tool_name(relation: RelationType, target: &str) -> String {
    match relation {
        RelationType::DelegatesTo => format!("delegate_to_{target}"),
        RelationType::Verifies => format!("verify_with_{target}"),
        RelationType::EscalatesTo => format!("escalate_to_{target}"),
    }
}

/// MCP tool descriptors visible to one node: one per outgoing edge, plus the report tool
/// for delegated (non-root) instances.
pub fn tool_descriptors(graph: &OrchestrationGraph, node_id: &str, delegated: bool) -> Vec<Value> {
    let mut tools = Vec::new();
    for edge in graph.edges.iter().filter(|edge| edge.source == node_id) {
        let target = graph.nodes.iter().find(|node| node.id == edge.target);
        let target_name = target.map(|node| node.data.name.as_str()).unwrap_or(&edge.target);
        let isolation = match target.map(|node| node.data.worktree_mode) {
            Some(WorktreeMode::Isolated) => {
                " Runs in an isolated worktree branched from your last commit, so commit before delegating."
            }
            _ => " Runs in your shared worktree, so edits are visible live.",
        };
        let relation = edge.data.relation_type;
        let (description, schema) = match relation {
            RelationType::DelegatesTo => (
                format!(
                    "Delegate a scoped task to {target_name} ({}). Blocks until that agent finishes and returns its summary, exit code and diff stat.{isolation}",
                    edge.target
                ),
                json!({
                    "type": "object",
                    "properties": {
                        "task": { "type": "string", "description": "Precise, self-contained task." },
                        "acceptance_criteria": { "type": "string", "description": "How the result will be judged." },
                        "target_files": { "type": "array", "items": { "type": "string" } },
                        "task_id": { "type": "string", "description": TASK_ID_DESCRIPTION }
                    },
                    "required": ["task"]
                }),
            ),
            RelationType::Verifies => (
                format!(
                    "Hand your current changes to {target_name} ({}) for verification. Blocks until the verifier reports pass/fail with evidence.{isolation}",
                    edge.target
                ),
                json!({
                    "type": "object",
                    "properties": {
                        "changes_summary": { "type": "string" },
                        "test_command": { "type": "string" },
                        "task_id": { "type": "string", "description": TASK_ID_DESCRIPTION }
                    },
                    "required": ["changes_summary"]
                }),
            ),
            RelationType::EscalatesTo => (
                format!(
                    "Escalate a blocker or a completed diff to {target_name} ({}). Blocks until a decision is returned.{isolation}",
                    edge.target
                ),
                json!({
                    "type": "object",
                    "properties": {
                        "blocker_description": { "type": "string" },
                        "context": { "type": "string" }
                    },
                    "required": ["blocker_description"]
                }),
            ),
        };
        tools.push(json!({
            "name": edge_tool_name(relation, &edge.target),
            "description": description,
            "inputSchema": schema,
        }));
    }

    if !delegated {
        tools.push(json!({
            "name": PLAN_TOOL,
            "description": "Set the plan the run follows: the tasks in order, and what each one depends on. Delegate each task by its id once the tasks it depends on are accepted. Tasks that have started keep their state. Tasks that have not started and are left out of the new plan are dropped.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "tasks": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": { "type": "string", "description": TASK_ID_DESCRIPTION },
                                "title": { "type": "string" },
                                "depends_on": { "type": "array", "items": { "type": "string" } }
                            },
                            "required": ["id", "title"]
                        }
                    }
                },
                "required": ["tasks"]
            }
        }));
    }

    if delegated {
        tools.push(json!({
            "name": REPORT_TOOL,
            "description": "Call exactly once when your assigned work is finished or blocked. This ends your session. The repository's gates then check your work, and a 'completed' report only stands if they pass.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "status": { "type": "string", "enum": ["completed", "blocked", "failed"] },
                    "summary": { "type": "string", "description": "What changed, and the evidence it works." },
                    "changed_files": { "type": "array", "items": { "type": "string" }, "description": "Files you changed, relative to the repository root." },
                    "commands_run": {
                        "type": "array",
                        "description": "The checks you ran, with their exit codes.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "command": { "type": "string" },
                                "exit_code": { "type": "integer" },
                                "excerpt": { "type": "string", "description": "The last relevant lines of output." }
                            },
                            "required": ["command"]
                        }
                    },
                    "open_questions": { "type": "array", "items": { "type": "string" } },
                    "failure_signature": { "type": "string", "description": "For a blocked or failed report: a short, stable description of the failure, such as the failing test name." }
                },
                "required": ["status", "summary"]
            }
        }));
    }
    tools
}

/// Builds the ephemeral system instructions: role, mandate, skills, scope and tools.
pub fn instructions(graph: &OrchestrationGraph, node: &GraphNode, repo_root: &Path, delegated: bool) -> String {
    let mut out = String::new();
    out.push_str(&format!("# Role: {}\n", node.data.name));
    if !node.data.role.trim().is_empty() {
        out.push_str(&format!("{}\n", node.data.role.trim()));
    }
    out.push('\n');

    if !node.data.system_prompt.trim().is_empty() {
        out.push_str("## Mandate\n");
        out.push_str(node.data.system_prompt.trim());
        out.push_str("\n\n");
    }

    out.push_str("## Scope\n");
    out.push_str("- Your working directory is your assigned git worktree. Stay inside it.\n");
    if node.data.context_paths.is_empty() {
        out.push_str("- No restricted context paths: use judgement within the worktree.\n");
    } else {
        out.push_str(&format!(
            "- Focus on these context paths: {}\n",
            node.data.context_paths.join(", ")
        ));
        out.push_str("- Read the rule files among them (AGENTS.md, CLAUDE.md, GEMINI.md) before editing.\n");
    }
    out.push('\n');

    out.push_str(&topology_section(graph, node));

    for skill in &node.data.skills {
        out.push_str(&format!("## Skill: {skill}\n"));
        match resolve_skill(repo_root, skill) {
            Some(text) => out.push_str(text.trim()),
            None => out.push_str(&format!(
                "(No skill file found at .orchestrator/skills/{skill}.md; apply the name as guidance.)"
            )),
        }
        out.push_str("\n\n");
    }

    let tools = tool_descriptors(graph, &node.id, delegated);
    if !tools.is_empty() {
        out.push_str("## Delegation tools\n");
        for tool in &tools {
            let name = tool["name"].as_str().unwrap_or_default();
            let description = tool["description"].as_str().unwrap_or_default();
            out.push_str(&format!("- `{name}`: {description}\n"));
        }
        out.push('\n');
    }

    if delegated {
        out.push_str("## Completion\n");
        out.push_str("When your task is finished or blocked, call `report_task_result` exactly once with a status and a concise summary. Also list the files you changed and the checks you ran with their exit codes. The repository's gates check your work after you report, and a 'completed' report only stands if they pass. Do not end without calling it.\n");
    } else {
        out.push_str("## Orchestration\n");
        if has_delegate_routes(graph, &node.id) {
            out.push_str(ROOT_DELEGATION_RULES);
            // Only a root that really starts in plan mode is told so; other providers have no such mode.
            if starts_in_plan_mode(graph, node) {
                out.push_str(ROOT_PLAN_MODE_RULE);
            } else {
                out.push_str(ROOT_PLAN_FIRST_RULE);
            }
            out.push_str(ROOT_DELEGATION_TAIL);
        } else {
            out.push_str("You are the root orchestrator. Decompose the user's goal into scoped tasks and summarize results for the user.\n");
        }
    }
    out
}

/// The root's mandate when it has a team to delegate to: the root coordinates and the team does
/// the work. These are rules, not advice, because a root that can run commands and edit files
/// will otherwise do small jobs itself, which skips the team and its verification.
const ROOT_DELEGATION_RULES: &str = "You are the root orchestrator. Your team is the agents in the Topology above, and they do the work. You coordinate them.\n\
- Any goal that changes something is delegated: code, configuration, dependencies, installs, global tools, or anything outside this repository. Use the delegate_to_* tools for that work, and do not run those commands or edit those files yourself, even when the task looks small.\n\
- Investigations that need commands or a search of the repository are delegated too. Answer the user yourself only from files you can read or from results the team has reported.\n\
";

const ROOT_PLAN_MODE_RULE: &str = "- You start in plan mode. Read what you need, then propose a plan: which team member takes each task, and in what order. Once the user approves it, record a multi-step plan with plan_tasks, then delegate each task by its id.\n";

/// For a root whose provider has no plan mode: the same plan-then-approve flow, asked for in words.
const ROOT_PLAN_FIRST_RULE: &str = "- Before you delegate, read what you need, then propose a plan: which team member takes each task, and in what order. Once the user approves it, record a multi-step plan with plan_tasks, then delegate each task by its id.\n";

const ROOT_DELEGATION_TAIL: &str = "- If no team member fits a task, tell the user what is missing and ask how to proceed. Do not do the work yourself.\n\
- Summarize each delegated result for the user: what changed, and whether its checks passed.\n";

/// Whether this node has a `delegates_to` route, so there is a team to hand work to.
fn has_delegate_routes(graph: &OrchestrationGraph, node_id: &str) -> bool {
    graph
        .edges
        .iter()
        .any(|edge| edge.source == node_id && edge.data.relation_type == RelationType::DelegatesTo)
}

/// The CEO starts in plan mode: a Claude root with a team to delegate to. It proposes its plan and
/// waits for the user's approval before it acts, so delegation waits on that approval too. A root
/// with no team is a single agent and starts as it always did. Other providers have no verified
/// plan mode here, so they are asked for the same flow in their instructions instead.
pub fn starts_in_plan_mode(graph: &OrchestrationGraph, node: &GraphNode) -> bool {
    node.data.provider == AgentProvider::Claude && has_delegate_routes(graph, &node.id)
}

/// Every agent in the run and every route between them, so each agent knows the whole team it
/// belongs to. Routes leaving this agent are its callable tools. The rest are context only.
fn topology_section(graph: &OrchestrationGraph, node: &GraphNode) -> String {
    let name_of = |id: &str| {
        graph
            .nodes
            .iter()
            .find(|member| member.id == id)
            .map(|member| member.data.name.clone())
            .unwrap_or_else(|| id.to_string())
    };
    let mut out = String::from("## Topology\nThis run's team. You are marked (you).\n");
    for member in &graph.nodes {
        let model = member.data.model.trim();
        let model = if model.is_empty() { String::new() } else { format!(" {model}") };
        let role = member.data.role.trim();
        let role = if role.is_empty() { "no role mandate" } else { role };
        let you = if member.id == node.id { " (you)" } else { "" };
        out.push_str(&format!(
            "- {} (`{}`, {}{}): {}{}\n",
            member.data.name,
            member.id,
            member.data.provider.binary(),
            model,
            role,
            you
        ));
    }
    if graph.edges.is_empty() {
        out.push_str("Routes: none.\n");
    } else {
        out.push_str("Routes. Routes from you are the tools above. The others are context only.\n");
        for edge in &graph.edges {
            let from_you = if edge.source == node.id { " (your tool)" } else { "" };
            out.push_str(&format!(
                "- {} {} {}{}\n",
                name_of(&edge.source),
                relation_phrase(edge.data.relation_type),
                name_of(&edge.target),
                from_you
            ));
        }
    }
    out.push('\n');
    out
}

fn relation_phrase(relation: RelationType) -> &'static str {
    match relation {
        RelationType::DelegatesTo => "delegates to",
        RelationType::Verifies => "verifies with",
        RelationType::EscalatesTo => "escalates to",
    }
}

/// `<repo>/.orchestrator/skills/<name>.md`, falling back to `~/.condurence/skills/<name>.md`.
pub fn resolve_skill(repo_root: &Path, name: &str) -> Option<String> {
    let safe = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !safe {
        return None;
    }
    let mut candidates = vec![repo_root.join(".orchestrator").join("skills").join(format!("{name}.md"))];
    if let Some(data) = crate::runtime::data_dir() {
        candidates.push(data.join("skills").join(format!("{name}.md")));
    }
    candidates.iter().find_map(|path| std::fs::read_to_string(path).ok())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Full TUI the user can converse with (the root orchestrator).
    Interactive,
    /// Non-interactive, machine-readable execution for delegated children.
    Headless,
}

pub struct BridgeAddress<'a> {
    pub exe: &'a Path,
    pub port: u16,
    pub run_id: &'a str,
    pub instance_id: &'a str,
    pub token: &'a str,
}

/// Everything needed to launch a provider CLI in a PTY.
#[derive(Clone, Debug, PartialEq)]
pub struct Invocation {
    /// Provider binary name, resolved against the hydrated PATH by the caller.
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// A file the CLI writes its final reply to, when it can. Reading it is more reliable than
    /// scraping the tail of a terminal log.
    pub result_file: Option<PathBuf>,
    /// Files and directories made for this one session. They hold the run's bearer token, so the
    /// router deletes them as soon as the session ends instead of leaving them until the run stops.
    pub cleanup: Vec<PathBuf>,
}

fn bridge_args(bridge: &BridgeAddress) -> Vec<String> {
    vec![
        "--mcp-bridge".into(),
        "--port".into(),
        bridge.port.to_string(),
        "--run-id".into(),
        bridge.run_id.into(),
        "--instance-id".into(),
        bridge.instance_id.into(),
        "--token".into(),
        bridge.token.into(),
    ]
}

/// Compiles the launch command for one provider. Adapters are kept in one place so that
/// flag differences between CLIs stay contained here.
pub fn invocation(
    provider: AgentProvider,
    mode: Mode,
    model: &str,
    effort: &str,
    system: &str,
    prompt: &str,
    bridge: &BridgeAddress,
    run_dir: &Path,
) -> AppResult<Invocation> {
    invocation_with_access(provider, mode, model, effort, system, prompt, bridge, run_dir, AccessLevel::ReadWrite, false)
}

/// Edit and shell tools a read-only Claude session is not allowed to use. Shell is included,
/// because a shell command can write as easily as an edit. The gates run the checks instead.
const CLAUDE_READ_ONLY_DENIED: &str = "--disallowedTools=Bash,Edit,Write,NotebookEdit";

/// [`invocation`] for a node with the given access. A read-only node gets the provider's read-only
/// mode where the CLI has one. Gemini and OpenCode have none that is verified here, so for them
/// the runtime's worktree check is the only enforcement. `plan` starts an interactive Claude session
/// in its plan mode; the other providers have no verified equivalent and ignore it.
#[allow(clippy::too_many_arguments)]
pub fn invocation_with_access(
    provider: AgentProvider,
    mode: Mode,
    model: &str,
    effort: &str,
    system: &str,
    prompt: &str,
    bridge: &BridgeAddress,
    run_dir: &Path,
    access: AccessLevel,
    plan: bool,
) -> AppResult<Invocation> {
    let read_only = access == AccessLevel::ReadOnly;
    std::fs::create_dir_all(run_dir)?;
    // Ephemeral, auditable copy of exactly what the agent was told (HANDOFF §6.3).
    std::fs::write(run_dir.join(format!("{}-instructions.md", bridge.instance_id)), system)?;
    let exe = bridge.exe.to_string_lossy().into_owned();
    let stem = format!("{}.{}", bridge.instance_id, provider.binary());
    let server = json!({ "command": exe, "args": bridge_args(bridge) });

    match provider {
        AgentProvider::Claude => {
            let config = run_dir.join(format!("{stem}-mcp.json"));
            std::fs::write(&config, serde_json::to_string_pretty(&json!({ "mcpServers": { SERVER_KEY: server } }))?)?;
            let mut args = Vec::new();
            match mode {
                Mode::Interactive => {
                    if !prompt.is_empty() {
                        args.push(prompt.to_string());
                    }
                    // Plan mode: the session reads and proposes, and asks before it edits or runs anything.
                    if plan {
                        args.extend(["--permission-mode".into(), "plan".into()]);
                    }
                }
                Mode::Headless => {
                    args.extend(["-p".into(), prompt.to_string()]);
                    args.extend(
                        ["--output-format", "stream-json", "--verbose", "--permission-mode", "acceptEdits"]
                            .map(String::from),
                    );
                }
            }
            push_model(&mut args, model, "--model");
            if !effort.is_empty() {
                args.extend(["--effort".into(), effort.to_string()]);
            }
            // Headless runs cannot answer permission prompts, and `acceptEdits` does not cover MCP
            // tools, so without this the child's `report_task_result` is silently refused. The
            // user wired these edges themselves, and approval gates are enforced by the router.
            // The `=` form keeps the variadic flag from swallowing the arguments after it.
            if read_only {
                args.push(CLAUDE_READ_ONLY_DENIED.into());
            }
            args.push(format!("--allowedTools=mcp__{SERVER_KEY}"));
            args.extend(["--mcp-config".into(), config.to_string_lossy().into_owned()]);
            if !system.is_empty() {
                args.extend(["--append-system-prompt".into(), system.to_string()]);
            }
            Ok(Invocation {
                program: provider.binary().into(),
                args,
                env: vec![],
                result_file: None,
                cleanup: vec![config],
            })
        }
        AgentProvider::Codex => {
            // Codex takes MCP servers as `-c key=value` overrides, each value parsed as TOML.
            let full = join_prompt(system, prompt);
            let mut args = Vec::new();
            match mode {
                Mode::Interactive => {
                    if !full.is_empty() {
                        args.push(full);
                    }
                }
                Mode::Headless => {
                    let sandbox = if read_only { "read-only" } else { "workspace-write" };
                    args.extend(["exec".into(), full, "-s".into(), sandbox.into()]);
                }
            }
            if read_only && mode == Mode::Interactive {
                args.extend(["-s".into(), "read-only".into()]);
            }
            // Codex prints a token counter after its reply, so the end of its terminal log is not
            // the answer. `-o` writes the final message alone to a file the router reads instead.
            let result_file = (mode == Mode::Headless).then(|| run_dir.join(format!("{}-last-message.txt", bridge.instance_id)));
            if let Some(file) = &result_file {
                args.extend(["-o".into(), file.to_string_lossy().into_owned()]);
            }
            push_model(&mut args, model, "-m");
            if !effort.is_empty() {
                args.extend(["-c".into(), format!("model_reasoning_effort={}", toml_string(effort))]);
            }
            let arg_list: Vec<String> = bridge_args(bridge).iter().map(|a| toml_string(a)).collect();
            // Codex asks before every MCP tool call, and headless children run with approval policy
            // `never`, so an unapproved call is refused. Without this, `report_task_result` never
            // lands and the child cannot report back. The user wired these edges, so the
            // orchestrator's tools are pre-approved, as they are for Claude.
            args.extend([
                "-c".into(),
                format!("mcp_servers.{SERVER_KEY}.command={}", toml_string(&exe)),
                "-c".into(),
                format!("mcp_servers.{SERVER_KEY}.args=[{}]", arg_list.join(",")),
                "-c".into(),
                format!("mcp_servers.{SERVER_KEY}.default_tools_approval_mode={}", toml_string("approve")),
                "-c".into(),
                format!("mcp_servers.{SERVER_KEY}.tool_timeout_sec={CODEX_TOOL_TIMEOUT_SECS}"),
            ]);
            Ok(Invocation {
                program: provider.binary().into(),
                args,
                env: vec![],
                result_file,
                cleanup: vec![],
            })
        }
        AgentProvider::Gemini => {
            // This provider runs Google's Antigravity CLI (`agy`), which replaced the Gemini CLI
            // after Google retired its Sign in with Google for it. `agy` has no documented
            // GEMINI_CLI_HOME equivalent and reads MCP servers from mcp_config.json, not
            // settings.json, so the home written below does not reach it: delegated Gemini runs
            // currently start without the bridge.
            let home = run_dir.join(format!("{stem}-home"));
            let user_gemini = crate::runtime::home_dir().map(|dir| dir.join(".gemini"));
            prepare_gemini_home(&home.join(".gemini"), user_gemini.as_deref(), &server)?;
            let full = join_prompt(system, prompt);
            let mut args = Vec::new();
            match mode {
                Mode::Interactive => {
                    if !full.is_empty() {
                        args.extend(["-i".into(), full]);
                    }
                }
                Mode::Headless => {
                    args.extend(["-p".into(), full, "--mode".into(), "accept-edits".into()]);
                }
            }
            // agy has no trust flag: workspace trust is asked once, on first launch.
            push_model(&mut args, model, "--model");
            Ok(Invocation {
                program: provider.binary().into(),
                args,
                env: vec![("GEMINI_CLI_HOME".into(), home.to_string_lossy().into_owned())],
                result_file: None,
                cleanup: vec![home],
            })
        }
        AgentProvider::Opencode => {
            // OPENCODE_CONFIG is merged on top of the user's own configuration, so their
            // providers and credentials stay in effect.
            let config = run_dir.join(format!("{stem}.json"));
            let server_local = json!({
                "type": "local",
                "command": std::iter::once(exe.clone()).chain(bridge_args(bridge)).collect::<Vec<_>>(),
                "enabled": true
            });
            std::fs::write(
                &config,
                serde_json::to_string_pretty(&json!({
                    "$schema": "https://opencode.ai/config.json",
                    "mcp": { SERVER_KEY: server_local }
                }))?,
            )?;
            let full = join_prompt(system, prompt);
            let env = vec![("OPENCODE_CONFIG".into(), config.to_string_lossy().into_owned())];
            let mut args = Vec::new();
            match mode {
                Mode::Interactive => {
                    if !full.is_empty() {
                        args.extend(["--prompt".into(), full]);
                    }
                }
                Mode::Headless => {
                    args.extend(["run".into(), full]);
                }
            }
            push_model(&mut args, model, "-m");
            if mode == Mode::Headless {
                // --auto: a headless child has no operator to answer permission prompts.
                args.push("--auto".into());
            }
            Ok(Invocation {
                program: provider.binary().into(),
                args,
                env,
                result_file: None,
                cleanup: vec![config],
            })
        }
    }
}

/// Builds the ephemeral `.gemini` directory for one Gemini instance. Every entry of the user's
/// real directory is symlinked (credentials are shared, never copied) except settings.json,
/// which is merged with the orchestrator's MCP server.
pub fn prepare_gemini_home(target: &Path, user_dir: Option<&Path>, server: &Value) -> AppResult<()> {
    std::fs::create_dir_all(target)?;
    let mut settings = json!({});
    if let Some(user_dir) = user_dir {
        if let Ok(entries) = std::fs::read_dir(user_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                if name == "settings.json" {
                    if let Ok(text) = std::fs::read_to_string(entry.path()) {
                        settings = serde_json::from_str(&text).unwrap_or_else(|_| json!({}));
                    }
                    continue;
                }
                link_into(&entry.path(), &target.join(&name))?;
            }
        }
    }
    if !settings.is_object() {
        settings = json!({});
    }
    let mut servers = settings
        .get("mcpServers")
        .cloned()
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    servers[SERVER_KEY] = server.clone();
    settings["mcpServers"] = servers;
    std::fs::write(target.join("settings.json"), serde_json::to_string_pretty(&settings)?)?;
    Ok(())
}

#[cfg(unix)]
fn link_into(source: &Path, link: &Path) -> AppResult<()> {
    if std::fs::symlink_metadata(link).is_err() {
        std::os::unix::fs::symlink(source, link)?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn link_into(_source: &Path, _link: &Path) -> AppResult<()> {
    // Windows symlinks need extra privileges. Without them the child starts signed out of
    // Gemini; this is a documented limitation rather than a silent copy of credentials.
    Ok(())
}

fn push_model(args: &mut Vec<String>, model: &str, flag: &str) {
    let model = model.trim();
    if !model.is_empty() {
        args.push(flag.into());
        args.push(model.into());
    }
}

/// Providers without a dedicated system-prompt flag receive instructions inline.
fn join_prompt(system: &str, prompt: &str) -> String {
    match (system.trim().is_empty(), prompt.trim().is_empty()) {
        (true, _) => prompt.to_string(),
        (false, true) => system.to_string(),
        (false, false) => format!("{system}\n\n---\n\n{prompt}"),
    }
}

fn toml_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::default_graph;

    #[test]
    fn a_root_with_a_team_is_told_to_delegate_rather_than_do_the_work() {
        let graph = default_graph();
        let ceo = graph.nodes.iter().find(|n| n.id == "ceo").unwrap();
        let text = instructions(&graph, ceo, Path::new("/repo"), false);
        assert!(text.contains("Any goal that changes something is delegated"), "{text}");
        assert!(text.contains("do not run those commands or edit those files yourself"), "{text}");
        // The case that went wrong: a global install is outside the repo, but still goes to the team.
        assert!(text.contains("installs, global tools, or anything outside this repository"), "{text}");
        assert!(text.contains("You start in plan mode"), "{text}");
        assert!(!text.contains("Decompose the user's goal into scoped tasks and summarize"), "{text}");
    }

    #[test]
    fn a_non_claude_root_is_not_told_it_starts_in_plan_mode() {
        let mut graph = default_graph();
        let ceo = graph.nodes.iter_mut().find(|n| n.id == "ceo").unwrap();
        ceo.data.provider = AgentProvider::Codex;
        let ceo = graph.nodes.iter().find(|n| n.id == "ceo").unwrap();
        assert!(!starts_in_plan_mode(&graph, ceo));
        let text = instructions(&graph, ceo, Path::new("/repo"), false);
        assert!(!text.contains("You start in plan mode"), "{text}");
        assert!(text.contains("propose a plan"), "{text}");
        assert!(text.contains("Summarize each delegated result"), "{text}");
    }

    #[test]
    fn a_root_without_a_team_is_not_told_to_delegate() {
        let mut graph = default_graph();
        graph.edges.clear();
        let ceo = graph.nodes.iter().find(|n| n.id == "ceo").unwrap();
        let text = instructions(&graph, ceo, Path::new("/repo"), false);
        assert!(!text.contains("Any goal that changes something is delegated"), "{text}");
        assert!(text.contains("You are the root orchestrator."), "{text}");
    }

    #[test]
    fn the_ceo_starts_in_claude_plan_mode_and_a_lone_root_or_a_child_does_not() {
        let dir = tempfile::tempdir().unwrap();
        let exe = PathBuf::from("/bin/condurence");
        let bridge = bridge_for(&exe);
        let plan_flag = |args: &[String]| args.windows(2).any(|pair| pair[0] == "--permission-mode" && pair[1] == "plan");

        let graph = default_graph();
        let ceo = graph.nodes.iter().find(|n| n.id == "ceo").unwrap();
        assert!(starts_in_plan_mode(&graph, ceo));
        let planned = invocation_with_access(
            AgentProvider::Claude,
            Mode::Interactive,
            "",
            "",
            "SYS",
            "goal",
            &bridge,
            dir.path(),
            AccessLevel::ReadWrite,
            starts_in_plan_mode(&graph, ceo),
        )
        .unwrap();
        assert!(plan_flag(&planned.args), "{:?}", planned.args);
        // The goal stays the session's first positional argument, as every interactive launch has it.
        assert_eq!(planned.args[0], "goal", "{:?}", planned.args);

        let senior = graph.nodes.iter().find(|n| n.id == "senior-dev").unwrap();
        assert!(!starts_in_plan_mode(&graph, senior));
        let child = invocation(AgentProvider::Claude, Mode::Headless, "", "", "SYS", "do it", &bridge, dir.path()).unwrap();
        assert!(!plan_flag(&child.args), "{:?}", child.args);

        let mut alone = default_graph();
        alone.edges.clear();
        let lone_root = alone.nodes.iter().find(|n| n.id == "ceo").unwrap();
        assert!(!starts_in_plan_mode(&alone, lone_root));
        let plain = invocation(AgentProvider::Claude, Mode::Interactive, "", "", "SYS", "goal", &bridge, dir.path()).unwrap();
        assert!(!plan_flag(&plain.args), "{:?}", plain.args);
    }

    #[test]
    fn every_agent_is_told_the_whole_team_and_its_place_in_it() {
        let graph = default_graph();
        let node = graph.nodes.iter().find(|n| n.id == "senior-dev").unwrap();
        let text = instructions(&graph, node, Path::new("/repo"), true);
        assert!(text.contains("## Topology"), "{text}");
        // The Debugger is not a tool of Senior Dev, but Senior Dev still has to know it exists.
        assert!(text.contains("Debugger & QA"), "{text}");
        assert!(text.contains("CEO / Architect"), "{text}");
        assert!(text.contains("Senior Developer (`senior-dev`"), "{text}");
        assert!(text.contains("(you)"), "{text}");
        assert!(text.contains("(your tool)"), "{text}");
        assert!(text.contains("verifies with"), "{text}");
    }

    #[test]
    fn edges_compile_to_scoped_tools() {
        let graph = default_graph();
        let ceo = tool_descriptors(&graph, "ceo", false);
        assert_eq!(ceo.len(), 2, "the orchestrator delegates and plans");
        assert_eq!(ceo[0]["name"], "delegate_to_senior-dev");
        assert!(ceo.iter().any(|t| t["name"] == PLAN_TOOL));

        let senior = tool_descriptors(&graph, "senior-dev", true);
        assert_eq!(senior[0]["name"], "verify_with_debugger");
        assert!(senior.iter().any(|t| t["name"] == REPORT_TOOL));

        let debugger = tool_descriptors(&graph, "debugger", false);
        assert_eq!(debugger[0]["name"], "escalate_to_ceo");
        assert!(!debugger.iter().any(|t| t["name"] == REPORT_TOOL));
    }

    #[test]
    fn claude_headless_uses_stream_json_and_mcp_config() {
        let dir = tempfile::tempdir().unwrap();
        let exe = PathBuf::from("/bin/condurence");
        let bridge = BridgeAddress {
            exe: &exe,
            port: 4000,
            run_id: "run1",
            instance_id: "senior-dev.1",
            token: "t",
        };
        let inv = invocation(AgentProvider::Claude, Mode::Headless, "sonnet", "", "SYS", "do it", &bridge, dir.path()).unwrap();
        assert_eq!(inv.program, "claude");
        assert_eq!(inv.args[0], "-p");
        assert!(inv.args.contains(&"stream-json".to_string()));
        assert!(inv.args.contains(&"--mcp-config".to_string()));
        let config = dir.path().join("senior-dev.1.claude-mcp.json");
        let json: Value = serde_json::from_str(&std::fs::read_to_string(config).unwrap()).unwrap();
        assert_eq!(json["mcpServers"][SERVER_KEY]["args"][0], "--mcp-bridge");
    }

    #[test]
    fn effort_is_passed_to_claude_and_codex_and_omitted_when_empty() {
        let dir = tempfile::tempdir().unwrap();
        let bridge = BridgeAddress { exe: Path::new("/bin/oc"), port: 1, run_id: "r", instance_id: "n.1", token: "t" };

        let claude = invocation(AgentProvider::Claude, Mode::Headless, "claude-haiku-5-5", "low", "SYS", "hi", &bridge, dir.path()).unwrap();
        let at = claude.args.iter().position(|a| a == "--effort").expect("--effort present");
        assert_eq!(claude.args[at + 1], "low");
        let model = claude.args.iter().position(|a| a == "--model").unwrap();
        assert_eq!(claude.args[model + 1], "claude-haiku-5-5");

        let codex = invocation(AgentProvider::Codex, Mode::Headless, "", "low", "SYS", "hi", &bridge, dir.path()).unwrap();
        assert!(codex.args.iter().any(|a| a == "model_reasoning_effort=\"low\""), "{:?}", codex.args);

        // The orchestrator's own MCP tools are pre-allowed so headless children can report back.
        let allowed = format!("--allowedTools=mcp__{SERVER_KEY}");
        assert!(claude.args.iter().any(|a| *a == allowed), "{:?}", claude.args);

        let plain = invocation(AgentProvider::Claude, Mode::Headless, "", "", "SYS", "hi", &bridge, dir.path()).unwrap();
        assert!(!plain.args.iter().any(|a| a == "--effort"));
    }

    #[test]
    fn codex_pre_approves_the_orchestrator_tools_in_both_modes() {
        let dir = tempfile::tempdir().unwrap();
        let exe = PathBuf::from("/bin/condurence");
        let bridge = BridgeAddress { exe: &exe, port: 4000, run_id: "r", instance_id: "n.1", token: "t" };
        let wanted = format!("mcp_servers.{SERVER_KEY}.default_tools_approval_mode=\"approve\"");
        for mode in [Mode::Headless, Mode::Interactive] {
            let codex = invocation(AgentProvider::Codex, mode, "", "", "SYS", "hi", &bridge, dir.path()).unwrap();
            assert!(codex.args.iter().any(|a| *a == wanted), "{mode:?}: {:?}", codex.args);
        }
    }

    #[test]
    fn codex_waits_longer_than_its_five_minute_default_for_a_delegation() {
        let dir = tempfile::tempdir().unwrap();
        let exe = PathBuf::from("/bin/condurence");
        let bridge = BridgeAddress { exe: &exe, port: 4000, run_id: "r", instance_id: "n.1", token: "t" };
        let wanted = format!("mcp_servers.{SERVER_KEY}.tool_timeout_sec={CODEX_TOOL_TIMEOUT_SECS}");
        // The root and every intermediate agent can be the one waiting on a child.
        for mode in [Mode::Headless, Mode::Interactive] {
            let codex = invocation(AgentProvider::Codex, mode, "", "", "SYS", "hi", &bridge, dir.path()).unwrap();
            assert!(codex.args.iter().any(|a| *a == wanted), "{mode:?}: {:?}", codex.args);
        }
        assert!(CODEX_TOOL_TIMEOUT_SECS > 300, "must exceed Codex's 300s default");
    }

    #[test]
    fn headless_codex_writes_its_final_reply_to_a_file_the_router_can_read() {
        let dir = tempfile::tempdir().unwrap();
        let exe = PathBuf::from("/bin/condurence");
        let bridge = BridgeAddress { exe: &exe, port: 4000, run_id: "r", instance_id: "n.1", token: "t" };

        let headless = invocation(AgentProvider::Codex, Mode::Headless, "", "", "SYS", "hi", &bridge, dir.path()).unwrap();
        let file = headless.result_file.expect("a headless Codex run reports through a file");
        let at = headless.args.iter().position(|a| a == "-o").expect("-o is passed");
        assert_eq!(headless.args[at + 1], file.to_string_lossy());

        // The root is an interactive TUI whose answer goes to the user, and other CLIs have no such flag.
        let interactive = invocation(AgentProvider::Codex, Mode::Interactive, "", "", "SYS", "hi", &bridge, dir.path()).unwrap();
        assert!(interactive.result_file.is_none() && !interactive.args.contains(&"-o".to_string()));
        let claude = invocation(AgentProvider::Claude, Mode::Headless, "", "", "SYS", "hi", &bridge, dir.path()).unwrap();
        assert!(claude.result_file.is_none());
    }

    #[test]
    fn files_that_hold_the_run_token_are_listed_for_removal_when_the_session_ends() {
        let dir = tempfile::tempdir().unwrap();
        let exe = PathBuf::from("/bin/condurence");
        let bridge = BridgeAddress { exe: &exe, port: 4000, run_id: "r", instance_id: "n.1", token: "secret-token" };
        for provider in [AgentProvider::Claude, AgentProvider::Gemini, AgentProvider::Opencode] {
            let inv = invocation(provider, Mode::Headless, "", "", "SYS", "hi", &bridge, dir.path()).unwrap();
            assert!(!inv.cleanup.is_empty(), "{provider:?} writes token files that must be removed");
            for path in &inv.cleanup {
                assert!(path.exists(), "{provider:?}: {path:?} should exist until the session ends");
                assert!(path.starts_with(dir.path()), "session files stay inside the run directory");
            }
        }
        // Codex carries the server in its arguments and has no token file.
        let codex = invocation(AgentProvider::Codex, Mode::Headless, "", "", "SYS", "hi", &bridge, dir.path()).unwrap();
        assert!(codex.cleanup.is_empty());
    }

    #[test]
    fn opencode_modes_use_documented_flags() {
        let dir = tempfile::tempdir().unwrap();
        let exe = PathBuf::from("/bin/condurence");
        let bridge = BridgeAddress {
            exe: &exe,
            port: 4000,
            run_id: "run1",
            instance_id: "senior-dev.1",
            token: "t",
        };
        let headless = invocation(AgentProvider::Opencode, Mode::Headless, "anthropic/sonnet", "", "SYS", "do it", &bridge, dir.path()).unwrap();
        assert_eq!(headless.args[0], "run");
        assert!(headless.args.contains(&"--auto".to_string()), "headless runs must auto-approve");
        assert!(headless.env.iter().any(|(key, _)| key == "OPENCODE_CONFIG"));

        let interactive = invocation(AgentProvider::Opencode, Mode::Interactive, "", "", "SYS", "goal", &bridge, dir.path()).unwrap();
        assert_eq!(interactive.args[0], "--prompt");
        assert!(!interactive.args.contains(&"--auto".to_string()));
    }

    #[test]
    fn gemini_runs_use_the_antigravity_cli_flags() {
        let dir = tempfile::tempdir().unwrap();
        let exe = PathBuf::from("/bin/condurence");
        let bridge = BridgeAddress {
            exe: &exe,
            port: 4000,
            run_id: "run1",
            instance_id: "senior-dev.1",
            token: "t",
        };
        let headless = invocation(AgentProvider::Gemini, Mode::Headless, "gemini-x", "", "SYS", "do it", &bridge, dir.path()).unwrap();
        assert_eq!(headless.program, "agy");
        assert_eq!(headless.args[0], "-p");
        assert!(headless.args.windows(2).any(|pair| pair == ["--mode", "accept-edits"]), "headless runs auto-accept edits");
        assert!(headless.args.windows(2).any(|pair| pair == ["--model", "gemini-x"]));
        assert!(!headless.args.contains(&"--skip-trust".to_string()), "agy has no --skip-trust");

        let interactive = invocation(AgentProvider::Gemini, Mode::Interactive, "", "", "SYS", "do it", &bridge, dir.path()).unwrap();
        assert_eq!(interactive.args[0], "-i");
        assert!(!interactive.args.contains(&"--mode".to_string()));
    }

    #[cfg(unix)]
    #[test]
    fn gemini_home_links_credentials_and_merges_mcp_settings() {
        let user = tempfile::tempdir().unwrap();
        let user_gemini = user.path().join(".gemini");
        std::fs::create_dir_all(&user_gemini).unwrap();
        std::fs::write(user_gemini.join("google_accounts.json"), r#"{"active":"me@example.com"}"#).unwrap();
        std::fs::write(
            user_gemini.join("settings.json"),
            r#"{"model":{"name":"gemini-x"},"mcpServers":{"other":{"command":"other-tool"}}}"#,
        )
        .unwrap();

        let run = tempfile::tempdir().unwrap();
        let target = run.path().join("home").join(".gemini");
        let server = json!({ "command": "/bin/condurence", "args": ["--mcp-bridge"] });
        prepare_gemini_home(&target, Some(&user_gemini), &server).unwrap();

        // Credentials are linked, not copied, so token refreshes land in the user's real file.
        let linked = std::fs::symlink_metadata(target.join("google_accounts.json")).unwrap();
        assert!(linked.file_type().is_symlink());
        assert!(std::fs::read_to_string(target.join("google_accounts.json")).unwrap().contains("me@example.com"));

        // Settings are a merged copy: the user's keys survive and the orchestrator is added.
        let settings: Value = serde_json::from_str(&std::fs::read_to_string(target.join("settings.json")).unwrap()).unwrap();
        assert_eq!(settings["model"]["name"], "gemini-x");
        assert_eq!(settings["mcpServers"]["other"]["command"], "other-tool");
        assert_eq!(settings["mcpServers"][SERVER_KEY]["command"], "/bin/condurence");
        assert!(!std::fs::symlink_metadata(target.join("settings.json")).unwrap().file_type().is_symlink());
    }

    #[test]
    fn toml_strings_are_escaped() {
        assert_eq!(toml_string(r#"a"b\c"#), r#""a\"b\\c""#);
    }

    fn bridge_for<'a>(exe: &'a Path) -> BridgeAddress<'a> {
        BridgeAddress {
            exe,
            port: 4000,
            run_id: "run1",
            instance_id: "debugger.1",
            token: "t",
        }
    }

    #[test]
    fn read_only_claude_cannot_use_edit_or_shell_tools() {
        let dir = tempfile::tempdir().unwrap();
        let exe = PathBuf::from("/bin/condurence");
        let bridge = bridge_for(&exe);
        let read_only = invocation_with_access(
            AgentProvider::Claude,
            Mode::Headless,
            "",
            "",
            "SYS",
            "review",
            &bridge,
            dir.path(),
            AccessLevel::ReadOnly,
            false,
        )
        .unwrap();
        assert!(read_only.args.contains(&CLAUDE_READ_ONLY_DENIED.to_string()));
        // The orchestrator's own tools stay available: a read-only reviewer still reports.
        assert!(read_only.args.contains(&format!("--allowedTools=mcp__{SERVER_KEY}")));

        let writable = invocation(AgentProvider::Claude, Mode::Headless, "", "", "SYS", "build", &bridge, dir.path()).unwrap();
        assert!(!writable.args.iter().any(|a| a.starts_with("--disallowedTools")));
    }

    #[test]
    fn read_only_codex_runs_in_the_read_only_sandbox() {
        let dir = tempfile::tempdir().unwrap();
        let exe = PathBuf::from("/bin/condurence");
        let bridge = bridge_for(&exe);
        let sandbox_of = |args: &[String]| {
            let at = args.iter().position(|a| a == "-s").expect("a sandbox flag");
            args[at + 1].clone()
        };
        let headless = invocation_with_access(
            AgentProvider::Codex,
            Mode::Headless,
            "",
            "",
            "SYS",
            "review",
            &bridge,
            dir.path(),
            AccessLevel::ReadOnly,
            false,
        )
        .unwrap();
        assert_eq!(sandbox_of(&headless.args), "read-only");

        let interactive = invocation_with_access(
            AgentProvider::Codex,
            Mode::Interactive,
            "",
            "",
            "SYS",
            "plan",
            &bridge,
            dir.path(),
            AccessLevel::ReadOnly,
            false,
        )
        .unwrap();
        assert_eq!(sandbox_of(&interactive.args), "read-only");

        let writable = invocation(AgentProvider::Codex, Mode::Headless, "", "", "SYS", "build", &bridge, dir.path()).unwrap();
        assert_eq!(sandbox_of(&writable.args), "workspace-write");
    }

    #[test]
    fn access_defaults_to_read_write_in_json() {
        let node: crate::graph::AgentNodeData = serde_json::from_value(json!({
            "name": "Dev",
            "worktreeMode": "isolated"
        }))
        .unwrap();
        assert_eq!(node.access, AccessLevel::ReadWrite);
        let reviewer: crate::graph::AgentNodeData = serde_json::from_value(json!({
            "name": "QA",
            "access": "read-only"
        }))
        .unwrap();
        assert_eq!(reviewer.access, AccessLevel::ReadOnly);
    }
}
