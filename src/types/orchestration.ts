/**
 * Canonical topology and runtime contracts (HANDOFF §6.1, §6.4).
 * Mirrors `src-tauri/src/graph/mod.rs`, `src-tauri/src/git/*` and `src-tauri/src/runtime/*`.
 * Keep the two sides in sync; the Rust side is the source of truth for serialization.
 */

import type { PullRequestStatus } from "./pulls";

export type AgentProvider = "claude" | "codex" | "opencode" | "gemini";
export type WorktreeMode = "shared" | "isolated";

/** `read-only` nodes may read and report, but not edit. The runtime checks their worktree afterwards. */
export type AccessLevel = "read-write" | "read-only";
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
  access: AccessLevel;
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
  gemini: "Antigravity CLI",
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

export type RunStatus = "running" | "finished" | "stopped" | "interrupted";

/** Per-repository Git settings, as on Conductor's Git page. Mirrors `RepoGitSettings` in `src-tauri/src/settings/mod.rs`. */
export type RepoGitSettings = {
  /** Remote-tracking branch that new workspaces start from, such as `origin/main`. */
  baseRef: string;
  /** Remote that receives pushes and PRs. */
  remote: string;
  deleteBranchOnArchive: boolean;
  archiveOnMerge: boolean;
  setUpstreamOnPush: boolean;
};

/** One orchestration run in the history. Mirrors `RunRecord` in `src-tauri/src/runtime/state_db.rs`. */
export type RunRecord = {
  id: string;
  repoPath: string;
  rootNodeId: string;
  goal: string;
  status: RunStatus;
  startedMs: number;
  endedMs: number | null;
};

/** One rate-limit window of a subscription plan. Mirrors `PlanWindow` in `src-tauri/src/usage/mod.rs`. */
export type PlanWindow = {
  /** Share of the window's allowance used, 0 to 100. */
  usedPercent: number;
  windowMinutes: number | null;
  /** Epoch seconds. Past it, `usedPercent` is out of date. */
  resetsAt: number | null;
};

/** The subscription's own usage meter, as the CLI last reported it (for example ChatGPT Free). */
export type PlanLimit = {
  plan: string | null;
  primary: PlanWindow | null;
  secondary: PlanWindow | null;
  /** True when the CLI reported that a limit was hit. */
  reached: boolean;
  /** Epoch ms of the report. */
  reportedMs: number;
};

/** Token usage one provider's CLI recorded for a run. Mirrors `src-tauri/src/usage/mod.rs`. */
export type ProviderUsage = {
  provider: AgentProvider;
  /** False when that CLI's logs are not read yet, so the token counts are zero and not meaningful. */
  measured: boolean;
  /** Session files attributed to the run. */
  sessions: number;
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheWriteTokens: number;
  /** The plan's usage meter, for providers whose logs carry one. Absent or null when none was reported. */
  planLimit?: PlanLimit | null;
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

/** What the All files tab lists for a workspace. Mirrors `git::files::FileListing`. */
export type FileListing = {
  /** Workspace-relative paths with `/` separators, sorted. */
  files: string[];
  /** The workspace has more files than were listed. */
  truncated: boolean;
};

/** One file's text. `content` is empty for a binary file and cut at `size` > 1 MB. */
export type FileContent = {
  path: string;
  content: string;
  binary: boolean;
  truncated: boolean;
  size: number;
};

/** What archiving a workspace would discard and keep. Mirrors `workspaces::ArchivePreview`. */
export type ArchivePreview = {
  /** Files with changes that are not committed. Archiving discards them. */
  uncommitted: string[];
  /** Commits on the branch that the remote has no copy of. They stay on the branch. */
  unpushed: number;
  /** The branch's PR, when GitHub could be asked and it has one. */
  pr: PullRequestStatus | null;
  /** The PR is merged and the branch holds nothing newer, so deleting the branch loses nothing. */
  delivered: boolean;
};

/** What happened to the branch. Mirrors `workspaces::ArchiveOutcome`. */
export type ArchiveOutcome = {
  branchDeleted: boolean;
  /** Why the branch was kept although the settings ask to delete it. */
  branchNote: string | null;
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
