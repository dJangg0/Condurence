import { useCallback, useEffect, useState } from "react";
import { LAUNCHERS } from "./NewWorkspaceDialog";
import { TerminalPane } from "./TerminalPane";
import { DiffDetail, FileList } from "./DiffViewer";
import { events, ipc, newSessionId } from "../lib/ipc";
import type { AgentProvider, FileDiff, WorktreeInfo } from "../types/orchestration";
import type { PullRequestStatus } from "../types/pulls";
import { BASE_BRANCH, pullButton, type PullAction, type PullButton } from "../lib/pullRequest";

type Notice = { tone: "info" | "error"; text: string };
export type Session = { id: string; cwd: string; label: string };

/** How often the selected workspace's PR is re-read, since mergeability and checks change on GitHub. */
const PR_POLL_MS = 20_000;

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

function basename(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
}

type Props = {
  repoPath: string;
  /** The repository's worktrees. The sidebar lists them; this view works on the selected one. */
  worktrees: WorktreeInfo[];
  /** The selected worktree path, or `null` to use the main checkout. */
  activePath: string | null;
  onSelect: (path: string | null) => void;
  /** Reloads the worktree list after a merge or removal and returns the fresh list. */
  onWorktreesChanged: () => Promise<WorktreeInfo[]>;
  session: Session | null;
  onSessionChange: (session: Session | null) => void;
};

/** Diff review, merge/remove, and a PTY terminal scoped to the selected workspace. */
export function WorkspaceView({
  repoPath,
  worktrees,
  activePath: selectedPath,
  onSelect,
  onWorktreesChanged,
  session,
  onSessionChange,
}: Props) {
  const activePath = selectedPath ?? worktrees.find((wt) => wt.isMain)?.path ?? worktrees[0]?.path ?? null;
  const [files, setFiles] = useState<FileDiff[]>([]);
  const [selectedFile, setSelectedFile] = useState<string | null>(null);
  // `undefined` until the first read of the branch's PR finishes. `null` means the branch has no PR yet.
  const [pr, setPr] = useState<PullRequestStatus | null | undefined>(undefined);
  const [prError, setPrError] = useState<string | null>(null);
  const [notice, setNotice] = useState<Notice | null>(null);
  const [confirmRemove, setConfirmRemove] = useState(false);
  const [busy, setBusy] = useState(false);

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
    setConfirmRemove(false);
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

  const removeWorkspace = async (worktree: WorktreeInfo) => {
    if (!confirmRemove) {
      setConfirmRemove(true);
      return;
    }
    setBusy(true);
    try {
      await ipc.removeWorktree(repoPath, worktree.branch ?? "", basename(worktree.path));
      const list = await onWorktreesChanged();
      onSelect(list.find((wt) => wt.isMain)?.path ?? null);
      setNotice({ tone: "info", text: "Workspace removed. Its branch was kept unless it was fully merged." });
    } catch (error) {
      setNotice({ tone: "error", text: String(error) });
    } finally {
      setConfirmRemove(false);
      setBusy(false);
    }
  };

  const activeBranch = activeWorktree && !activeWorktree.isMain ? activeWorktree.branch : null;

  // GitHub's mergeability and checks change without any local event, so keep the PR state fresh
  // while this workspace is selected.
  useEffect(() => {
    setPr(undefined);
    setPrError(null);
    if (!activePath || !activeBranch) return;
    let cancelled = false;
    let timer: number | undefined;
    const poll = async () => {
      try {
        const next = await ipc.pullRequestStatus(activePath, activeBranch);
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
  }, [activePath, activeBranch]);

  /** Runs whatever the PR button currently offers: create the PR, resolve conflicts, or merge. */
  const runPullAction = async (action: PullAction) => {
    if (!activePath || !activeBranch) return;
    setBusy(true);
    try {
      if (action === "create") {
        const created = await ipc.createPullRequest(activePath, activeBranch, BASE_BRANCH);
        setPr(created);
        setNotice({ tone: "info", text: `Opened PR #${created.number}. Merge unlocks when the checks pass.` });
      } else if (action === "merge") {
        const merged = await ipc.mergePullRequest(activePath, activeBranch);
        setPr(merged);
        setNotice({ tone: "info", text: `Merged PR #${merged.number} into ${BASE_BRANCH}.` });
      } else {
        const outcome = await ipc.syncWithBase(activePath, activeBranch, BASE_BRANCH);
        if (outcome.conflicts.length > 0) {
          // Hand the conflicts to Claude in this workspace. The next Resolve press commits and pushes.
          const id = newSessionId();
          await ipc.startAgentTask(id, activePath, "resolve", BASE_BRANCH);
          onSessionChange({ id, cwd: activePath, label: "Resolve" });
          setNotice({ tone: "error", text: `${outcome.message} Conflicts: ${outcome.conflicts.join(", ")}.` });
        } else {
          setNotice({ tone: "info", text: outcome.message });
        }
        setPr(await ipc.pullRequestStatus(activePath, activeBranch));
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
      await ipc.startAgentTask(id, activePath, "review", BASE_BRANCH);
      onSessionChange({ id, cwd: activePath, label: "Review" });
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
      onSessionChange({ id, cwd: activePath, label });
    } catch (error) {
      setNotice({ tone: "error", text: String(error) });
    }
  };

  const sessionHere = session && session.cwd === activePath ? session : null;

  return (
    <div className="flex h-full min-h-0">
      <section className="flex min-w-0 flex-1 flex-col">
        <div className="flex flex-wrap items-center gap-2 border-b border-zinc-800 bg-zinc-900/70 px-4 py-2.5">
          <div className="min-w-0">
            <div className="truncate text-sm font-semibold text-zinc-100">
              {activeWorktree?.branch ?? "No workspace selected"}
            </div>
            <div className="truncate font-mono text-[11px] text-zinc-500">{activePath}</div>
          </div>
          <div className="ml-auto flex flex-wrap items-center gap-2">
            {activeWorktree && activeBranch ? (
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
                  title={`Open Claude on a review of this branch against ${BASE_BRANCH}`}
                  className="rounded-md border border-zinc-700 px-2.5 py-1.5 text-xs text-zinc-200 hover:bg-zinc-800 disabled:opacity-50"
                >
                  Review
                </button>
                <PullActionButton button={pullButton(pr)} busy={busy} onAction={(action) => void runPullAction(action)} />
                <button
                  type="button"
                  onClick={() => void removeWorkspace(activeWorktree)}
                  disabled={busy}
                  className={`rounded-md px-2.5 py-1.5 text-xs font-medium disabled:opacity-50 ${
                    confirmRemove ? "bg-red-600 text-white hover:bg-red-500" : "border border-red-500/40 text-red-300 hover:bg-red-500/10"
                  }`}
                >
                  {confirmRemove ? "Confirm remove" : "Remove"}
                </button>
              </>
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

        <div className="flex min-h-0 flex-1">
          <div className="w-72 shrink-0 overflow-y-auto border-r border-zinc-800">
            <div className="px-3 pb-1 pt-3 text-[11px] font-semibold uppercase tracking-wider text-zinc-500">
              Changes · {files.length}
            </div>
            <FileList files={files} selected={selectedFile} onSelect={setSelectedFile} />
          </div>
          <div className="min-w-0 flex-1 overflow-auto bg-zinc-950">
            <DiffDetail file={files.find((file) => file.path === selectedFile)} />
          </div>
        </div>

        <div className="flex h-72 shrink-0 flex-col border-t border-zinc-800">
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
            {sessionHere ? <span className="ml-auto text-[11px] text-zinc-500">{sessionHere.label} session</span> : null}
          </div>
          <div className="min-h-0 flex-1">
            {sessionHere ? (
              <TerminalPane key={sessionHere.id} sessionId={sessionHere.id} interactive />
            ) : (
              <div className="p-4 text-xs text-zinc-500">Launch a shell or an agent CLI scoped to this workspace.</div>
            )}
          </div>
        </div>
      </section>
    </div>
  );
}
