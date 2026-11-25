//! Graph-to-MCP compilation: turns a node's outgoing edges into MCP tools, writes the
//! ephemeral instruction file, and builds the per-provider launch command.
//!
//! Everything written here lives under the OS temp directory (never inside the tracked
//! repository) and is removed when the run stops.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::error::AppResult;
use crate::graph::{AgentProvider, GraphNode, OrchestrationGraph, RelationType, WorktreeMode};

/// MCP server key registered with every provider CLI.
pub const SERVER_KEY: &str = "openconductor";
/// Injected into delegated (headless) agents so they can hand back a structured result.
pub const REPORT_TOOL: &str = "report_task_result";

pub fn run_dir(run_id: &str) -> PathBuf {
    std::env::temp_dir().join("openconductor").join(run_id)
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
                        "target_files": { "type": "array", "items": { "type": "string" } }
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
                        "test_command": { "type": "string" }
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

    if delegated {
        tools.push(json!({
            "name": REPORT_TOOL,
            "description": "Call exactly once when your assigned work is finished or blocked. This ends your session and returns the summary to the agent that requested it.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "status": { "type": "string", "enum": ["completed", "blocked", "failed"] },
                    "summary": { "type": "string", "description": "What changed, and the evidence it works." }
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
        out.push_str("When your task is finished or blocked, call `report_task_result` exactly once with a status and a concise summary. Do not end without calling it.\n");
    } else {
        out.push_str("## Orchestration\n");
        out.push_str("You are the root orchestrator. Decompose the user's goal into scoped tasks, delegate them with the tools above, and summarize results for the user.\n");
    }
    out
}

/// `<repo>/.orchestrator/skills/<name>.md`, falling back to `~/.openconductor/skills/<name>.md`.
pub fn resolve_skill(repo_root: &Path, name: &str) -> Option<String> {
    let safe = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !safe {
        return None;
    }
    let mut candidates = vec![repo_root.join(".orchestrator").join("skills").join(format!("{name}.md"))];
    if let Some(home) = crate::runtime::home_dir() {
        candidates.push(home.join(".openconductor").join("skills").join(format!("{name}.md")));
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
    /// Typed into a TUI shortly after launch, for providers without a prompt flag.
    pub initial_input: Option<String>,
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
    system: &str,
    prompt: &str,
    bridge: &BridgeAddress,
    run_dir: &Path,
) -> AppResult<Invocation> {
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
            args.extend(["--mcp-config".into(), config.to_string_lossy().into_owned()]);
            if !system.is_empty() {
                args.extend(["--append-system-prompt".into(), system.to_string()]);
            }
            Ok(Invocation {
                program: provider.binary().into(),
                args,
                env: vec![],
                initial_input: None,
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
                    args.extend(["exec".into(), full, "-s".into(), "workspace-write".into()]);
                }
            }
            push_model(&mut args, model, "-m");
            let arg_list: Vec<String> = bridge_args(bridge).iter().map(|a| toml_string(a)).collect();
            args.extend([
                "-c".into(),
                format!("mcp_servers.{SERVER_KEY}.command={}", toml_string(&exe)),
                "-c".into(),
                format!("mcp_servers.{SERVER_KEY}.args=[{}]", arg_list.join(",")),
            ]);
            Ok(Invocation {
                program: provider.binary().into(),
                args,
                env: vec![],
                initial_input: None,
            })
        }
        AgentProvider::Gemini => {
            // Gemini reads MCP servers from ~/.gemini/settings.json; point its home at an
            // ephemeral directory so the user's own configuration is left untouched.
            let home = run_dir.join(format!("{stem}-home"));
            std::fs::create_dir_all(home.join(".gemini"))?;
            std::fs::write(
                home.join(".gemini").join("settings.json"),
                serde_json::to_string_pretty(&json!({ "mcpServers": { SERVER_KEY: server } }))?,
            )?;
            let full = join_prompt(system, prompt);
            let mut args = Vec::new();
            match mode {
                Mode::Interactive => {
                    if !full.is_empty() {
                        args.extend(["-i".into(), full]);
                    }
                }
                Mode::Headless => {
                    args.extend(["-p".into(), full, "--approval-mode".into(), "auto_edit".into()]);
                }
            }
            push_model(&mut args, model, "-m");
            Ok(Invocation {
                program: provider.binary().into(),
                args,
                env: vec![("GEMINI_CLI_HOME".into(), home.to_string_lossy().into_owned())],
                initial_input: None,
            })
        }
        AgentProvider::Opencode => {
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
            match mode {
                Mode::Interactive => Ok(Invocation {
                    program: provider.binary().into(),
                    args: vec![],
                    env,
                    initial_input: (!full.is_empty()).then_some(full),
                }),
                Mode::Headless => {
                    let mut args = vec!["run".into(), full];
                    push_model(&mut args, model, "-m");
                    Ok(Invocation {
                        program: provider.binary().into(),
                        args,
                        env,
                        initial_input: None,
                    })
                }
            }
        }
    }
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
    fn edges_compile_to_scoped_tools() {
        let graph = default_graph();
        let ceo = tool_descriptors(&graph, "ceo", false);
        assert_eq!(ceo.len(), 1);
        assert_eq!(ceo[0]["name"], "delegate_to_senior-dev");

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
        let exe = PathBuf::from("/bin/openconductor");
        let bridge = BridgeAddress {
            exe: &exe,
            port: 4000,
            run_id: "run1",
            instance_id: "senior-dev.1",
            token: "t",
        };
        let inv = invocation(AgentProvider::Claude, Mode::Headless, "sonnet", "SYS", "do it", &bridge, dir.path()).unwrap();
        assert_eq!(inv.program, "claude");
        assert_eq!(inv.args[0], "-p");
        assert!(inv.args.contains(&"stream-json".to_string()));
        assert!(inv.args.contains(&"--mcp-config".to_string()));
        let config = dir.path().join("senior-dev.1.claude-mcp.json");
        let json: Value = serde_json::from_str(&std::fs::read_to_string(config).unwrap()).unwrap();
        assert_eq!(json["mcpServers"]["openconductor"]["args"][0], "--mcp-bridge");
    }

    #[test]
    fn toml_strings_are_escaped() {
        assert_eq!(toml_string(r#"a"b\c"#), r#""a\"b\\c""#);
    }
}
