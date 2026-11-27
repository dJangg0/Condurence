//! Orchestration topology: the Git-tracked `<repo>/.orchestrator/graph.json`.
//!
//! The serialized shape is the canonical contract from HANDOFF §6.1 and must stay
//! in sync with `src/types/orchestration.ts`.

use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

pub const GRAPH_RELATIVE_PATH: &str = ".orchestrator/graph.json";
pub const MAX_NODE_ID_LEN: usize = 32;
pub const MAX_RETRIES_LIMIT: u32 = 20;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OrchestrationGraph {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub default_worktree_strategy: WorktreeMode,
    #[serde(default)]
    pub nodes: Vec<GraphNode>,
    #[serde(default)]
    pub edges: Vec<GraphEdge>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GraphNode {
    pub id: String,
    #[serde(rename = "type", default = "default_node_type")]
    pub kind: String,
    pub position: Position,
    pub data: AgentNodeData,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Default)]
pub struct Position {
    pub x: f64,
    pub y: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentNodeData {
    pub name: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub provider: AgentProvider,
    #[serde(default)]
    pub model: String,
    /// Reasoning effort for the provider CLI, such as `low`. Empty uses the CLI's default.
    #[serde(default)]
    pub effort: String,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub context_paths: Vec<String>,
    #[serde(default)]
    pub system_prompt: String,
    #[serde(default)]
    pub worktree_mode: WorktreeMode,
    /// `read-only` for reviewers and planners, which may read and report but not edit.
    #[serde(default)]
    pub access: AccessLevel,
    /// Runtime-only. Persisted as `idle` so topology diffs stay clean.
    #[serde(default)]
    pub status: NodeStatus,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GraphEdge {
    pub id: String,
    pub source: String,
    pub target: String,
    pub data: RoutingEdgeData,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RoutingEdgeData {
    pub relation_type: RelationType,
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    #[serde(default)]
    pub requires_approval: bool,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum AgentProvider {
    #[default]
    Claude,
    Codex,
    Opencode,
    Gemini,
}

impl AgentProvider {
    /// The CLI binary name used to launch this provider.
    pub fn binary(self) -> &'static str {
        match self {
            AgentProvider::Claude => "claude",
            AgentProvider::Codex => "codex",
            AgentProvider::Opencode => "opencode",
            AgentProvider::Gemini => "gemini",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum WorktreeMode {
    /// Reuse the upstream agent's worktree so edits are visible live.
    Shared,
    /// Provision a child `git worktree` branched from the upstream HEAD.
    #[default]
    Isolated,
}

/// What a node may change. A `read-only` node is launched in its provider's read-only mode where
/// the CLI has one, and the runtime checks that its worktree was left as it was found.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum AccessLevel {
    #[default]
    ReadWrite,
    ReadOnly,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum NodeStatus {
    #[default]
    Idle,
    Running,
    WaitingApproval,
    Error,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RelationType {
    DelegatesTo,
    Verifies,
    EscalatesTo,
}

fn default_version() -> u32 {
    1
}

fn default_node_type() -> String {
    "agentNode".to_string()
}

fn default_max_retries() -> u32 {
    3
}

/// Starter topology: a CEO orchestrator delegating to an isolated developer, who is
/// verified by a shared debugger that can escalate back to the CEO behind an approval gate.
pub fn default_graph() -> OrchestrationGraph {
    OrchestrationGraph {
        version: 1,
        default_worktree_strategy: WorktreeMode::Isolated,
        nodes: vec![
            GraphNode {
                id: "ceo".into(),
                kind: default_node_type(),
                position: Position { x: 260.0, y: 40.0 },
                data: AgentNodeData {
                    name: "CEO / Architect".into(),
                    role: "Primary Orchestrator".into(),
                    provider: AgentProvider::Claude,
                    model: "opus".into(),
                    effort: String::new(),
                    skills: vec!["spec-planner".into(), "task-router".into()],
                    context_paths: vec!["AGENTS.md".into(), "docs/".into()],
                    system_prompt: "Decompose user goals into scoped tasks and delegate via MCP tools.".into(),
                    worktree_mode: WorktreeMode::Shared,
                    access: AccessLevel::ReadWrite,
                    status: NodeStatus::Idle,
                },
            },
            GraphNode {
                id: "senior-dev".into(),
                kind: default_node_type(),
                position: Position { x: 80.0, y: 260.0 },
                data: AgentNodeData {
                    name: "Senior Developer".into(),
                    role: "Implementation".into(),
                    provider: AgentProvider::Claude,
                    model: "sonnet".into(),
                    effort: String::new(),
                    skills: vec![],
                    context_paths: vec!["src/".into()],
                    system_prompt: "Implement the delegated task. Keep changes small and commit when done.".into(),
                    worktree_mode: WorktreeMode::Isolated,
                    access: AccessLevel::ReadWrite,
                    status: NodeStatus::Idle,
                },
            },
            GraphNode {
                id: "debugger".into(),
                kind: default_node_type(),
                position: Position { x: 440.0, y: 260.0 },
                data: AgentNodeData {
                    name: "Debugger & QA".into(),
                    role: "Verification".into(),
                    provider: AgentProvider::Codex,
                    model: String::new(),
                    effort: String::new(),
                    skills: vec![],
                    context_paths: vec!["tests/".into()],
                    system_prompt: "Review the delegated changes against their acceptance criteria and the gate output. Isolate each failure and report it with evidence. You cannot edit files: send the fix back to the developer.".into(),
                    worktree_mode: WorktreeMode::Shared,
                    access: AccessLevel::ReadOnly,
                    status: NodeStatus::Idle,
                },
            },
        ],
        edges: vec![
            GraphEdge {
                id: "e-ceo-senior-dev".into(),
                source: "ceo".into(),
                target: "senior-dev".into(),
                data: RoutingEdgeData {
                    relation_type: RelationType::DelegatesTo,
                    max_retries: 3,
                    requires_approval: false,
                },
            },
            GraphEdge {
                id: "e-senior-dev-debugger".into(),
                source: "senior-dev".into(),
                target: "debugger".into(),
                data: RoutingEdgeData {
                    relation_type: RelationType::Verifies,
                    max_retries: 3,
                    requires_approval: false,
                },
            },
            GraphEdge {
                id: "e-debugger-ceo".into(),
                source: "debugger".into(),
                target: "ceo".into(),
                data: RoutingEdgeData {
                    relation_type: RelationType::EscalatesTo,
                    max_retries: 2,
                    requires_approval: true,
                },
            },
        ],
    }
}

/// Node ids are used verbatim in MCP tool names and Tauri event names, so they are
/// restricted to a conservative character set.
pub fn is_valid_node_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_NODE_ID_LEN
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Reasoning-effort levels accepted by the provider adapters. Empty means the CLI default.
pub const EFFORT_LEVELS: [&str; 7] = ["minimal", "low", "medium", "high", "xhigh", "max", "ultra"];

pub fn validate(graph: &OrchestrationGraph) -> AppResult<()> {
    let mut node_ids: HashSet<&str> = HashSet::new();
    for node in &graph.nodes {
        if !is_valid_node_id(&node.id) {
            crate::error::bail!(
                "Invalid node id '{}': use 1-{} letters, digits, '-' or '_'",
                node.id,
                MAX_NODE_ID_LEN
            );
        }
        if !node_ids.insert(node.id.as_str()) {
            crate::error::bail!("Duplicate node id '{}'", node.id);
        }
        if node.data.name.trim().is_empty() {
            crate::error::bail!("Node '{}' needs a name", node.id);
        }
        if !node.data.effort.is_empty() && !EFFORT_LEVELS.contains(&node.data.effort.as_str()) {
            crate::error::bail!(
                "Node '{}' has unknown effort '{}': use one of {}",
                node.id,
                node.data.effort,
                EFFORT_LEVELS.join(", ")
            );
        }
    }

    let mut edge_ids: HashSet<&str> = HashSet::new();
    for edge in &graph.edges {
        if !edge_ids.insert(edge.id.as_str()) {
            crate::error::bail!("Duplicate edge id '{}'", edge.id);
        }
        if !node_ids.contains(edge.source.as_str()) || !node_ids.contains(edge.target.as_str()) {
            crate::error::bail!("Edge '{}' references a missing node", edge.id);
        }
        if edge.source == edge.target {
            crate::error::bail!("Edge '{}' cannot connect a node to itself", edge.id);
        }
        if edge.data.max_retries > MAX_RETRIES_LIMIT {
            crate::error::bail!("Edge '{}' maxRetries must be <= {}", edge.id, MAX_RETRIES_LIMIT);
        }
    }
    Ok(())
}

/// Reads `<repo>/.orchestrator/graph.json`, or returns the starter topology when the
/// repository has not been configured yet.
pub fn load(repo_root: &Path) -> AppResult<OrchestrationGraph> {
    let path = repo_root.join(GRAPH_RELATIVE_PATH);
    if !path.is_file() {
        return Ok(default_graph());
    }
    let text = std::fs::read_to_string(&path)?;
    let graph: OrchestrationGraph = serde_json::from_str(&text)
        .map_err(|e| AppError::msg(format!("{} is not valid: {e}", path.display())))?;
    validate(&graph)?;
    Ok(graph)
}

/// Validates and atomically writes the topology. Runtime-only node status is reset to
/// `idle` so that saving never dirties Git with transient state.
pub fn save(repo_root: &Path, graph: &OrchestrationGraph) -> AppResult<()> {
    validate(graph)?;
    let mut persisted = graph.clone();
    for node in &mut persisted.nodes {
        node.data.status = NodeStatus::Idle;
    }

    let path = repo_root.join(GRAPH_RELATIVE_PATH);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut json = serde_json::to_string_pretty(&persisted)?;
    json.push('\n');
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effort_is_validated_and_defaults_to_empty_for_old_graph_files() {
        let mut graph = default_graph();
        graph.nodes[0].data.effort = "low".into();
        validate(&graph).expect("known effort is accepted");
        graph.nodes[0].data.effort = "turbo".into();
        let error = validate(&graph).unwrap_err().to_string();
        assert!(error.contains("unknown effort 'turbo'"), "{error}");

        // A graph.json written before the field existed still loads.
        let mut value = serde_json::to_value(default_graph()).unwrap();
        for node in value["nodes"].as_array_mut().unwrap() {
            node["data"].as_object_mut().unwrap().remove("effort");
        }
        let loaded: OrchestrationGraph = serde_json::from_value(value).unwrap();
        assert!(loaded.nodes.iter().all(|n| n.data.effort.is_empty()));
    }

    #[test]
    fn default_graph_is_valid_and_round_trips() {
        let graph = default_graph();
        validate(&graph).expect("default graph must validate");
        let json = serde_json::to_string(&graph).unwrap();
        let back: OrchestrationGraph = serde_json::from_str(&json).unwrap();
        assert_eq!(graph, back);
    }

    #[test]
    fn serializes_spec_shape() {
        let json: serde_json::Value = serde_json::to_value(default_graph()).unwrap();
        assert_eq!(json["defaultWorktreeStrategy"], "isolated");
        assert_eq!(json["nodes"][0]["type"], "agentNode");
        assert_eq!(json["nodes"][0]["data"]["contextPaths"][0], "AGENTS.md");
        assert_eq!(json["nodes"][0]["data"]["worktreeMode"], "shared");
        assert_eq!(json["edges"][0]["data"]["relationType"], "delegates_to");
        assert_eq!(json["edges"][0]["data"]["requiresApproval"], false);
        assert_eq!(json["edges"][2]["data"]["requiresApproval"], true);
    }

    #[test]
    fn rejects_dangling_edges_and_bad_ids() {
        let mut graph = default_graph();
        graph.edges[0].target = "ghost".into();
        assert!(validate(&graph).is_err());

        let mut graph = default_graph();
        graph.nodes[0].id = "has space".into();
        assert!(validate(&graph).is_err());
    }

    #[test]
    fn save_then_load_round_trips_and_resets_status() {
        let dir = tempfile::tempdir().unwrap();
        let mut graph = default_graph();
        graph.nodes[1].data.status = NodeStatus::Running;
        save(dir.path(), &graph).unwrap();
        let loaded = load(dir.path()).unwrap();
        assert_eq!(loaded.nodes[1].data.status, NodeStatus::Idle);
        assert_eq!(loaded.nodes.len(), 3);
    }
}
