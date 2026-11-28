import { useCallback, useEffect, useRef, useState } from "react";
import { LAUNCHERS } from "../lib/launchers";
import { TerminalPane } from "./TerminalPane";
import { PaneHandle } from "./PaneHandle";
import { DiffDetail, FileList } from "./DiffViewer";
import { events, ipc, newSessionId } from "../lib/ipc";
import type { AgentProvider, FileDiff, RepoGitSettings, WorktreeInfo } from "../types/orchestration";
import type { PullRequestStatus } from "../types/pulls";
import { pullButton, type PullAction, type PullButton } from "../lib/pullRequest";
import { archiveResult, workspaceFolder } from "../lib/archive";
import { DEFAULT_REPO_SETTINGS, baseBranchOf } from "../lib/repoSettings";
import { sessionsIn, shownSession } from "../lib/terminals";
import { openPullRequest } from "../lib/pullActions";
import { workspaceCommitMessage } from "../lib/changes";

type Notice = { tone: "info" | "error"; text: string };
export type Session = { id: string; cwd: string; label: string };

/** How often the selected workspace's PR is re-read, since mergeability and checks change on GitHub. */
const PR_POLL_MS = 20_000;

/** Starting sizes of the changes list and the terminal, and the smallest each may be dragged to. */
const DEFAULT_CHANGES_WIDTH = 288;
const DEFAULT_TERMINAL_HEIGHT = 288;
const MIN_CHANGES_WIDTH = 180;
const MIN_DIFF_WIDTH = 280;
const MIN_TERMINAL_HEIGHT = 120;
const MIN_DIFF_HEIGHT = 200;
/** Width of the vertical handle between the changes list and the diff. */
const HANDLE_WIDTH = 6;

const PULL_TONES: Record<PullButton["tone"], string> = {
  primary: "bg-indigo-600 text-white",
  success: "bg-emerald-600 text-white",
  warning: "bg-amber-600 text-white",
  danger: "bg-red-600 text-white",
  muted: "bg-zinc-800 text-zinc-300",
};

/** The PR button. It only does something when the state has an action (create, resolve or merge). */
function PullActionButton({
  button,
  busy,
  onAction,
}: {
  button: PullButton;
  busy: boolean;
  onAction: (action: PullAction) => void;
}) {
  return (
    <button
      type="button"
      title={button.hint}
      disabled={busy || button.action === null}
      onClick={() => {
        if (button.action) onAction(button.action);
      }}
      className={`rounded-md px-2.5 py-1.5 text-xs font-medium transition enabled:hover:brightness-110 disabled:cursor-default disabled:opacity-70 ${PULL_TONES[button.tone]}`}
    >
      {button.label}
    </button>
  );
}

type Props = {
  repoPath: string;
  /** The repository's worktrees. The sidebar lists them; this view works on the selected one. */
  worktrees: WorktreeInfo[];
  /** The selected worktree path, or `null` to use the main checkout. */
  activePath: string | null;
  /** Opens the archive confirmation for a workspace. Nothing is removed until it is confirmed there. */
  onArchive: (worktree: WorktreeInfo) => void;
  /** A workspace was archived here, so the lists and the selection should follow. */
  onArchived: (worktree: WorktreeInfo) => void;
  /** Every terminal session that is open, in any workspace. This view shows the ones in its workspace. */
  sessions: Session[];
  /** The session each workspace's terminal last showed, by workspace path. */
  selectedSessionIds: Record<string, string>;
  /** A terminal was started. It is added to the workspace's tabs, not in place of the others. */
  onSessionStart: (session: Session) => void;
  onSessionSelect: (cwd: string, id: string) => void;
  /** The session's tab was closed, which ends its process. */
  onSessionEnd: (id: string) => void;
  /** A PR was opened or merged here, so the repository's PR list, and the sidebar's merge states, are read again. */
  onPullChanged: () => void;
};

/** Diff review, PR and merge actions, and a PTY terminal scoped to the selected workspace. */
export function WorkspaceView({
  repoPath,
  worktrees,
  activePath: selectedPath,
  onArchive,
  onArchived,
  sessions,
  selectedSessionIds,
  onSessionStart,
  onSessionSelect,
  onSessionEnd,
  onPullChanged,
}: Props) {
  const activePath = selectedPath ?? worktrees.find((wt) => wt.isMain)?.path ?? worktrees[0]?.path ?? null;
  const [files, setFiles] = useState<FileDiff[]>([]);
  const [selectedFile, setSelectedFile] = useState<string | null>(null);
  // `undefined` until the first read of the branch's PR finishes. `null` means the branch has no PR yet.
  const [pr, setPr] = useState<PullRequestStatus | null | undefined>(undefined);
  const [prError, setPrError] = useState<string | null>(null);
  const [notice, setNotice] = useState<Notice | null>(null);
  const [busy, setBusy] = useState(false);
  // The repository's Git settings: the base branch, the remote for pushes and PRs, and what archiving
  // and merging do. Read again if the repository changes.
  const [settings, setSettings] = useState<RepoGitSettings>(DEFAULT_REPO_SETTINGS);
  const base = baseBranchOf(settings);
  // Sizes of the changes list and the terminal, changed by dragging their handles.
  const [changesWidth, setChangesWidth] = useState(DEFAULT_CHANGES_WIDTH);
  const [terminalHeight, setTerminalHeight] = useState(DEFAULT_TERMINAL_HEIGHT);
  const sectionRef = useRef<HTMLElement>(null);
  const rowRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    let cancelled = false;
    ipc
      .getRepoGitSettings(repoPath)
      .then((loaded) => {
        if (!cancelled) setSettings(loaded);
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [repoPath]);

  const loadDiff = useCallback(async (path: string) => {
    try {
      const diff = await ipc.getGitDiff(path);
      setFiles(diff);
      setSelectedFile((current) => (diff.some((file) => file.path === current) ? current : diff[0]?.path ?? null));
    } catch (error) {
      setNotice({ tone: "error", text: String(error) });
    }
  }, []);

  // Keep the diff live: the backend debounces filesystem bursts into one dirty event.
  useEffect(() => {
    if (!activePath) return;
    setFiles([]);
    setSelectedFile(null);
    void loadDiff(activePath);

    let disposed = false;
    let unlisten: (() => void) | undefined;
    void ipc.watchWorkspace(activePath).then((workspaceId) =>
      events.workspaceDirty(workspaceId, () => void loadDiff(activePath)).then((fn) => {
        if (disposed) fn();
        else unlisten = fn;
      }),
    );
    return () => {
      disposed = true;
      unlisten?.();
      void ipc.unwatchWorkspace(activePath).catch(() => undefined);
    };
  }, [activePath, loadDiff]);

  const activeWorktree = worktrees.find((wt) => wt.path === activePath);

  /**
   * Archives a workspace once its PR has merged, but only when the repository's "Archive on merge"
   * setting is on, which it is not by default. Uncommitted changes are never discarded this way: a
   * workspace that has any is left as it is and the reason is shown.
   */
  const archiveAfterMerge = async (worktree: WorktreeInfo): Promise<void> => {
    try {
      const outcome = await ipc.archiveWorkspace(repoPath, worktree.branch, workspaceFolder(worktree.path), false);
      onArchived(worktree);
      setNotice({ tone: "info", text: archiveResult(outcome, worktree.branch) });
    } catch (error) {
      setNotice({ tone: "error", text: `Merged, but the workspace was not archived. ${String(error)}` });
    }
  };

  // Every branch except the base can open a PR, including the main checkout's own feature branch.
  const activeBranch = activeWorktree?.branch ?? null;
  const prBranch = activeBranch && activeBranch !== base ? activeBranch : null;

  // GitHub's mergeability and checks change without any local event, so keep the PR state fresh
  // while this workspace is selected.
  useEffect(() => {
    setPr(undefined);
    setPrError(null);
    if (!activePath || !prBranch) return;
    let cancelled = false;
    let timer: number | undefined;
    const poll = async () => {
      try {
        const next = await ipc.pullRequestStatus(activePath, prBranch, settings.remote);
        if (!cancelled) {
          setPr(next);
          setPrError(null);
          // A merged PR never changes again, so stop spawning `gh` for it.
          if (next?.state === "MERGED") return;
        }
      } catch (error) {
        if (!cancelled) setPrError(String(error));
      }
      if (!cancelled) timer = window.setTimeout(() => void poll(), PR_POLL_MS);
    };
    void poll();
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [activePath, prBranch, settings.remote]);

  /** Runs whatever the PR button currently offers: create the PR, resolve conflicts, or merge. */
  const runPullAction = async (action: PullAction) => {
    if (!activePath || !prBranch) return;
    setBusy(true);
    try {
      if (action === "create") {
        // Pending edits are committed first, so the PR carries them. With nothing pending the commit is a no-op.
        const pending = files.length;
        const created = await openPullRequest({
          worktreePath: activePath,
          branch: prBranch,
          base,
          remote: settings.remote,
          commitMessage: workspaceCommitMessage(files.map((file) => file.path)),
        });
        setPr(created);
        onPullChanged();
        const committed = pending > 0 ? `Committed ${pending} ${pending === 1 ? "file" : "files"} and opened` : "Opened";
        setNotice({ tone: "info", text: `${committed} PR #${created.number}. Merge unlocks when the checks pass.` });
      } else if (action === "merge") {
        const merged = await ipc.mergePullRequest(activePath, prBranch, settings.remote);
        setPr(merged);
        onPullChanged();
        setNotice({ tone: "info", text: `Merged PR #${merged.number} into ${base}.` });
        // Archive on merge is opt-in. Left off, the workspace stays where it is after its PR merges.
        if (settings.archiveOnMerge && activeWorktree && !activeWorktree.isMain) {
          await archiveAfterMerge(activeWorktree);
        }
      } else {
        const outcome = await ipc.syncWithBase(activePath, prBranch, base, settings.remote);
        if (outcome.conflicts.length > 0) {
          // Hand the conflicts to Claude in this workspace. The next Resolve press commits and pushes.
          const id = newSessionId();
          await ipc.startAgentTask(id, activePath, "resolve", base);
          onSessionStart({ id, cwd: activePath, label: "Resolve" });
          setNotice({ tone: "error", text: `${outcome.message} Conflicts: ${outcome.conflicts.join(", ")}.` });
        } else {
          setNotice({ tone: "info", text: outcome.message });
        }
        setPr(await ipc.pullRequestStatus(activePath, prBranch, settings.remote));
      }
    } catch (error) {
      setNotice({ tone: "error", text: String(error) });
    } finally {
      setBusy(false);
    }
  };

  /** Opens Claude in this workspace on a review of the branch against the base. */
  const startReview = async () => {
    if (!activePath) return;
    const id = newSessionId();
    try {
      await ipc.startAgentTask(id, activePath, "review", base);
      onSessionStart({ id, cwd: activePath, label: "Review" });
    } catch (error) {
      setNotice({ tone: "error", text: String(error) });
    }
  };

  const launch = async (provider: AgentProvider | "shell") => {
    if (!activePath) return;
    const id = newSessionId();
    const label = LAUNCHERS.find((item) => item.id === provider)?.label ?? provider;
    try {
      await ipc.spawnAgentPty(id, activePath, provider, 120, 30);
      onSessionStart({ id, cwd: activePath, label });
    } catch (error) {
      setNotice({ tone: "error", text: String(error) });
    }
  };

  // Only this workspace's sessions get tabs. Sessions in other workspaces keep running untouched.
  const here = sessionsIn(sessions, activePath);
  const shown = shownSession(sessions, activePath ? selectedSessionIds[activePath] : undefined, activePath);

  return (
    <div className="flex h-full min-h-0">
      <section ref={sectionRef} className="flex min-w-0 flex-1 flex-col">
        <div className="flex flex-wrap items-center gap-2 border-b border-zinc-800 bg-zinc-900/70 px-4 py-2.5">
          <div className="min-w-0">
            <div className="truncate text-sm font-semibold text-zinc-100">
              {activeWorktree?.branch ?? "No workspace selected"}
            </div>
            <div className="truncate font-mono text-[11px] text-zinc-500">{activePath}</div>
          </div>
          <div className="ml-auto flex flex-wrap items-center gap-2">
            {activeWorktree && prBranch ? (
              <>
                {pr ? (
                  <span className="text-[11px] text-zinc-500" title={pr.url}>
                    PR #{pr.number}
                  </span>
                ) : null}
                {prError ? (
                  <span className="text-[11px] text-red-300" title={prError}>
                    PR status unavailable
                  </span>
                ) : null}
                <button
                  type="button"
                  onClick={() => void startReview()}
                  disabled={!activePath}
                  title={`Open Claude on a review of this branch against ${base}`}
                  className="rounded-md border border-zinc-700 px-2.5 py-1.5 text-xs text-zinc-200 hover:bg-zinc-800 disabled:opacity-50"
                >
                  Review
                </button>
                <PullActionButton button={pullButton(pr, base)} busy={busy} onAction={(action) => void runPullAction(action)} />
              </>
            ) : null}
            {activeWorktree && !activeWorktree.isMain ? (
              <button
                type="button"
                onClick={() => onArchive(activeWorktree)}
                disabled={busy}
                title="Archive this workspace. A merged or open PR does not do it for you."
                className="rounded-md border border-zinc-700 px-2.5 py-1.5 text-xs font-medium text-zinc-200 hover:bg-zinc-800 disabled:opacity-50"
              >
                Archive
              </button>
            ) : null}
          </div>
        </div>

        {notice ? (
          <div
            className={`border-b px-4 py-1.5 text-xs ${
              notice.tone === "error" ? "border-red-500/30 bg-red-500/10 text-red-200" : "border-zinc-800 bg-zinc-900 text-zinc-400"
            }`}
          >
            {notice.text}
          </div>
        ) : null}

        <div ref={rowRef} className="flex min-h-0 flex-1">
          <div className="shrink-0 overflow-y-auto" style={{ width: changesWidth }}>
            <div className="px-3 pb-1 pt-3 text-[11px] font-semibold uppercase tracking-wider text-zinc-500">
              Changes · {files.length}
            </div>
            <FileList files={files} selected={selectedFile} onSelect={setSelectedFile} />
          </div>
          <PaneHandle
            axis="x"
            growsToward="end"
            label="Resize changes list width"
            size={changesWidth}
            onResize={setChangesWidth}
            bounds={() => ({
              min: MIN_CHANGES_WIDTH,
              max: (rowRef.current?.clientWidth ?? 0) - HANDLE_WIDTH - MIN_DIFF_WIDTH,
            })}
          />
          <div className="min-w-0 flex-1 overflow-auto bg-zinc-950">
            <DiffDetail file={files.find((file) => file.path === selectedFile)} />
          </div>
        </div>

        <PaneHandle
          axis="y"
          label="Resize terminal"
          size={terminalHeight}
          onResize={setTerminalHeight}
          bounds={() => ({
            min: MIN_TERMINAL_HEIGHT,
            // Growing the terminal shrinks the diff area above it, one pixel for one, down to its minimum.
            max: terminalHeight + (rowRef.current?.clientHeight ?? 0) - MIN_DIFF_HEIGHT,
          })}
        />
        <div className="flex shrink-0 flex-col max-h-[calc(100%-12rem)]" style={{ height: terminalHeight }}>
          <div className="flex flex-wrap items-center gap-1.5 border-b border-zinc-800 px-3 py-1.5">
            <span className="mr-1 text-xs font-medium text-zinc-300">Terminal</span>
            {LAUNCHERS.map((launcher) => (
              <button
                key={launcher.id}
                type="button"
                onClick={() => void launch(launcher.id)}
                disabled={!activePath}
                className="rounded-md bg-zinc-800 px-2 py-1 text-[11px] text-zinc-200 hover:bg-zinc-700 disabled:opacity-40"
              >
                {launcher.label}
              </button>
            ))}
            {here.length > 0 ? (
              <div className="ml-auto flex min-w-0 flex-wrap items-center gap-1">
                {here.map((item) => {
                  const selected = item.id === shown?.id;
                  return (
                    <div
                      key={item.id}
                      className={`flex items-center rounded-md text-[11px] ${selected ? "bg-zinc-700 text-zinc-100" : "bg-zinc-800/60 text-zinc-400 hover:bg-zinc-800"}`}
                    >
                      <button
                        type="button"
                        aria-pressed={selected}
                        onClick={() => {
                          if (activePath) onSessionSelect(activePath, item.id);
                        }}
                        className="px-2 py-1"
                      >
                        {item.label}
                      </button>
                      <button
                        type="button"
                        title="End this session"
                        aria-label={`End ${item.label} session`}
                        onClick={() => onSessionEnd(item.id)}
                        className="rounded-r-md px-1.5 py-1 text-zinc-500 hover:text-zinc-200"
                      >
                        ×
                      </button>
                    </div>
                  );
                })}
              </div>
            ) : null}
          </div>
          <div className="min-h-0 flex-1">
            {/* Every session here stays mounted and only the selected one is shown. Remounting would replay the
                backend's byte tail and lose the output above it, so a tab switch must not remount. */}
            {here.map((item) => (
              <div key={item.id} className={item.id === shown?.id ? "h-full" : "hidden"}>
                <TerminalPane sessionId={item.id} interactive />
              </div>
            ))}
            {shown ? null : (
              <div className="p-4 text-xs text-zinc-500">Launch a shell or an agent CLI scoped to this workspace.</div>
            )}
          </div>
        </div>
      </section>
    </div>
  );
}
