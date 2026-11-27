import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  ApprovalRequestEvent,
  AgentProvider,
  ArchiveOutcome,
  ArchivePreview,
  FileContent,
  FileDiff,
  FileListing,
  MergeResult,
  NodeStatusEvent,
  OrchestrationGraph,
  ModelOption,
  ProviderUsage,
  RepoMetadata,
  RepoRecord,
  RunEvent,
  RunLogEvent,
  RepoGitSettings,
  RunRecord,
  WorktreeInfo,
} from "../types/orchestration";
import type { ProviderAuth } from "../types/providers";
import type { GitHubAccount, GitHubRepoConnection, GitHubRepoSummary } from "../types/github";
import type { BranchPull, PullRequestStatus, SyncOutcome } from "../types/pulls";
import type { ReviewContextPayload } from "./review";

/** Typed wrappers over the Tauri command surface (HANDOFF §6.4). */
export const ipc = {
  githubAccountStatus: () => invoke<GitHubAccount>("github_account_status"),
  githubRepoConnection: (repoPath: string) => invoke<GitHubRepoConnection>("github_repo_connection", { repoPath }),
  githubListRepositories: () => invoke<GitHubRepoSummary[]>("github_list_repositories"),
  githubConnectRepository: (repoPath: string, nameWithOwner: string) =>
    invoke<GitHubRepoConnection>("github_connect_repository", { repoPath, nameWithOwner }),
  githubStartSignIn: (sessionId: string) => invoke<void>("github_start_sign_in", { sessionId }),
  listProviderModels: (provider: AgentProvider) => invoke<ModelOption[]>("list_provider_models", { provider }),
  providerAuthStatus: () => invoke<ProviderAuth[]>("provider_auth_status"),
  providerStartSignIn: (sessionId: string, provider: string) =>
    invoke<void>("provider_start_sign_in", { sessionId, provider }),
  githubCloneRepository: (nameWithOwner: string) => invoke<string>("github_clone_repository", { nameWithOwner }),

  openRepository: (path: string) => invoke<RepoMetadata>("open_repository", { path }),
  listRecentRepositories: () => invoke<RepoRecord[]>("list_recent_repositories"),
  listArchivedRepositories: () => invoke<RepoRecord[]>("list_archived_repositories"),
  archiveRepository: (path: string, archived: boolean) => invoke<void>("archive_repository", { path, archived }),
  forgetRepository: (path: string) => invoke<void>("forget_repository", { path }),
  loadGraph: (repoPath: string) => invoke<OrchestrationGraph>("load_graph", { repoPath }),
  saveGraph: (repoPath: string, graph: OrchestrationGraph) => invoke<void>("save_graph", { repoPath, graph }),

  listWorktrees: (repoPath: string) => invoke<WorktreeInfo[]>("list_worktrees", { repoPath }),
  createWorktree: (repoPath: string, branchName: string, folderName: string, startPoint?: string) =>
    invoke<WorktreeInfo>("create_worktree", { repoPath, branchName, folderName, startPoint: startPoint ?? null }),
  archivePreview: (repoPath: string, worktreePath: string, branch: string | null) =>
    invoke<ArchivePreview>("archive_preview", { repoPath, worktreePath, branch }),
  archiveWorkspace: (repoPath: string, branch: string | null, folderName: string, discardChanges: boolean) =>
    invoke<ArchiveOutcome>("archive_workspace", { repoPath, branch, folderName, discardChanges }),
  mergeWorktree: (repoPath: string, branchName: string, targetBranch: string, squash: boolean) =>
    invoke<MergeResult>("merge_worktree", { repoPath, branchName, targetBranch, squash }),
  pullRequestStatus: (worktreePath: string, branch: string, remote: string) =>
    invoke<PullRequestStatus | null>("pull_request_status", { worktreePath, branch, remote }),
  listPullRequests: (repoPath: string) => invoke<BranchPull[]>("list_pull_requests", { repoPath }),
  createPullRequest: (worktreePath: string, branch: string, base: string, remote: string) =>
    invoke<PullRequestStatus>("create_pull_request", { worktreePath, branch, base, remote }),
  mergePullRequest: (worktreePath: string, branch: string, remote: string) =>
    invoke<PullRequestStatus>("merge_pull_request", { worktreePath, branch, remote }),
  syncWithBase: (worktreePath: string, branch: string, base: string, remote: string) =>
    invoke<SyncOutcome>("sync_with_base", { worktreePath, branch, base, remote }),
  createWorkspace: (repoPath: string) => invoke<WorktreeInfo>("create_workspace", { repoPath }),
  getRepoGitSettings: (repoPath: string) => invoke<RepoGitSettings>("get_repo_git_settings", { repoPath }),
  saveRepoGitSettings: (repoPath: string, settings: RepoGitSettings) =>
    invoke<void>("save_repo_git_settings", { repoPath, settings }),
  listRemotes: (repoPath: string) => invoke<string[]>("list_remotes", { repoPath }),
  listRemoteBranches: (repoPath: string, remote: string) =>
    invoke<string[]>("list_remote_branches", { repoPath, remote }),
  startAgentTask: (sessionId: string, worktreePath: string, task: "review" | "resolve", base: string) =>
    invoke<void>("start_agent_task", { sessionId, worktreePath, task, base }),
  getGitDiff: (worktreePath: string) => invoke<FileDiff[]>("get_git_diff", { worktreePath }),
  getBranchDiff: (worktreePath: string, base: string) => invoke<FileDiff[]>("get_branch_diff", { worktreePath, base }),
  listWorktreeFiles: (worktreePath: string) => invoke<FileListing>("list_worktree_files", { worktreePath }),
  readWorktreeFile: (worktreePath: string, path: string) =>
    invoke<FileContent>("read_worktree_file", { worktreePath, path }),
  startReview: (sessionId: string, worktreePath: string, base: string, agent: string, context: ReviewContextPayload) =>
    invoke<void>("start_review", { sessionId, worktreePath, base, agent, context }),
  watchWorkspace: (worktreePath: string) => invoke<string>("watch_workspace", { worktreePath }),
  unwatchWorkspace: (worktreePath: string) => invoke<void>("unwatch_workspace", { worktreePath }),

  spawnAgentPty: (sessionId: string, cwd: string, provider: AgentProvider | "shell", cols: number, rows: number) =>
    invoke<void>("spawn_agent_pty", { sessionId, cwd, provider, cols, rows }),
  writeAgentPty: (sessionId: string, data: string) => invoke<void>("write_agent_pty", { sessionId, data }),
  resizeAgentPty: (sessionId: string, cols: number, rows: number) =>
    invoke<void>("resize_agent_pty", { sessionId, cols, rows }),
  killAgentPty: (sessionId: string) => invoke<void>("kill_agent_pty", { sessionId }),
  getPtyHistory: (sessionId: string) => invoke<string>("get_pty_history", { sessionId }),

  startOrchestrationRun: (repoPath: string, rootNodeId: string, userGoal: string) =>
    invoke<string>("start_orchestration_run", { repoPath, rootNodeId, userGoal }),
  stopOrchestrationRun: (runId: string) => invoke<void>("stop_orchestration_run", { runId }),
  resolveApprovalGate: (runId: string, edgeId: string, approved: boolean) =>
    invoke<void>("resolve_approval_gate", { runId, edgeId, approved }),
  listRunEvents: (runId: string, limit?: number) => invoke<RunEvent[]>("list_run_events", { runId, limit: limit ?? null }),
  listRuns: (repoPath: string, limit?: number) => invoke<RunRecord[]>("list_runs", { repoPath, limit: limit ?? null }),
  runWorktrees: (runId: string) => invoke<WorktreeInfo[]>("run_worktrees", { runId }),
  startBranch: (worktreePath: string, branch: string) => invoke<void>("start_branch", { worktreePath, branch }),
  commitWorktreeChanges: (worktreePath: string, message: string) =>
    invoke<string | null>("commit_worktree_changes", { worktreePath, message }),
  runUsage: (runId: string) => invoke<ProviderUsage[]>("run_usage", { runId }),
  providerUsageSummary: () => invoke<ProviderUsage[]>("provider_usage_summary"),
};

/** Typed subscriptions over the runtime event bus (HANDOFF §6.4 events). */
export const events = {
  ptyOutput: (sessionId: string, onChunk: (chunk: string) => void): Promise<UnlistenFn> =>
    listen<string>(`pty-output://${sessionId}`, (event) => onChunk(event.payload)),
  ptyExit: (sessionId: string, onExit: (exitCode: number) => void): Promise<UnlistenFn> =>
    listen<{ exitCode: number }>(`pty-exit://${sessionId}`, (event) => onExit(event.payload.exitCode)),
  nodeStatus: (nodeId: string, onStatus: (payload: NodeStatusEvent) => void): Promise<UnlistenFn> =>
    listen<NodeStatusEvent>(`node-status://${nodeId}`, (event) => onStatus(event.payload)),
  workspaceDirty: (workspaceId: string, onDirty: (changedFilesCount: number) => void): Promise<UnlistenFn> =>
    listen<{ changedFilesCount: number }>(`workspace-dirty://${workspaceId}`, (event) =>
      onDirty(event.payload.changedFilesCount),
    ),
  runLog: (runId: string, onEntry: (entry: RunLogEvent) => void): Promise<UnlistenFn> =>
    listen<RunLogEvent>(`run-log://${runId}`, (event) => onEntry(event.payload)),
  approvalRequest: (runId: string, onRequest: (request: ApprovalRequestEvent) => void): Promise<UnlistenFn> =>
    listen<ApprovalRequestEvent>(`approval-request://${runId}`, (event) => onRequest(event.payload)),
};

/** Random, unguessable id for a terminal session. */
export function newSessionId(): string {
  return crypto.randomUUID();
}
