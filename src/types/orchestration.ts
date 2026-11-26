/**
 * Canonical topology and runtime contracts (HANDOFF §6.1, §6.4).
 * Mirrors `src-tauri/src/graph/mod.rs`, `src-tauri/src/git/*` and `src-tauri/src/runtime/*`.
 * Keep the two sides in sync; the Rust side is the source of truth for serialization.
 */

export type AgentProvider = "claude" | "codex" | "opencode" | "gemini";
export type WorktreeMode = "shared" | "isolated";
export type NodeStatus = "idle" | "running" | "waiting_approval" | "error";
export type RelationType = "delegates_to" | "verifies" | "escalates_to";

export type AgentNodeData = {
  name: string;
  role: string;
  provider: AgentProvider;
  model: string;
  /** Reasoning effort passed to the CLI, such as `low`. Empty uses the CLI's default. */
  effort: string;
  skills: string[];
  contextPaths: string[];
  systemPrompt: string;
  worktreeMode: WorktreeMode;
  /** Runtime-only. Always persisted as "idle". */
  status: NodeStatus;
};

export type RoutingEdgeData = {
  relationType: RelationType;
  maxRetries: number;
  requiresApproval: boolean;
};

export type GraphNode = {
  id: string;
  type: "agentNode";
  position: { x: number; y: number };
  data: AgentNodeData;
};

export type GraphEdge = {
  id: string;
  source: string;
  target: string;
  data: RoutingEdgeData;
};

export type OrchestrationGraph = {
  version: number;
  defaultWorktreeStrategy: WorktreeMode;
  nodes: GraphNode[];
  edges: GraphEdge[];
};

/** Effort levels the provider adapters accept. Mirrors `EFFORT_LEVELS` in `src-tauri/src/graph/mod.rs`. */
export const EFFORT_LEVELS = ["minimal", "low", "medium", "high", "xhigh", "max", "ultra"] as const;

/** A model the provider CLI offers, for the inspector's model dropdown. */
export type ModelOption = { id: string; label: string; note: string | null };

export const PROVIDERS: readonly AgentProvider[] = ["claude", "codex", "opencode", "gemini"];

export const PROVIDER_LABELS: Record<AgentProvider, string> = {
  claude: "Claude Code",
  codex: "Codex CLI",
  opencode: "OpenCode",
  gemini: "Gemini CLI",
};

export const RELATION_TYPES: readonly RelationType[] = ["delegates_to", "verifies", "escalates_to"];

export const RELATION_LABELS: Record<RelationType, string> = {
  delegates_to: "Delegates to",
  verifies: "Verifies",
  escalates_to: "Escalates to",
};

export const RELATION_HELP: Record<RelationType, string> = {
  delegates_to: "The source dispatches a scoped task. The target runs headless and returns its result.",
  verifies: "The source hands its changes over for verification. Loops are allowed, bounded by maxRetries.",
  escalates_to: "The source surfaces a blocker or a finished diff upward. Use approval gates for humans.",
};

// ---------------------------------------------------------------- runtime events

export type NodeStatusEvent = {
  runId: string;
  status: NodeStatus;
  summary?: string | null;
  sessionId?: string | null;
};

export type RunLogEvent = {
  ts: number;
  nodeId: string | null;
  kind: string;
  payload: Record<string, unknown>;
};

export type ApprovalRequestEvent = {
  edgeId: string;
  fromNodeId: string;
  targetNodeId: string;
  prompt: string;
};

export type RunEvent = {
  id: number;
  runId: string;
  nodeId: string | null;
  kind: string;
  payload: Record<string, unknown>;
  createdMs: number;
};

// ---------------------------------------------------------------- git & workspaces

export type RepoMetadata = {
  path: string;
  name: string;
  currentBranch: string;
  hasGraph: boolean;
};

export type RepoRecord = {
  path: string;
  name: string;
  lastOpenedMs: number;
  archived: boolean;
};

export type WorktreeInfo = {
  path: string;
  branch: string | null;
  head: string;
  isMain: boolean;
  locked: boolean;
};

export type MergeResult = {
  merged: boolean;
  commit: string | null;
  conflicts: string[];
  message: string;
};

export type FileStatus = "added" | "modified" | "deleted" | "renamed" | "untracked" | "conflicted";
export type LineKind = "context" | "add" | "del";

export type DiffLine = {
  kind: LineKind;
  oldLine: number | null;
  newLine: number | null;
  content: string;
};

export type DiffHunk = {
  header: string;
  oldStart: number;
  oldLines: number;
  newStart: number;
  newLines: number;
  lines: DiffLine[];
};

export type FileDiff = {
  path: string;
  oldPath: string | null;
  status: FileStatus;
  additions: number;
  deletions: number;
  binary: boolean;
  hunks: DiffHunk[];
};
