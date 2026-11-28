import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { DiffDetail, FileList } from "./DiffViewer";
import { FilesPane } from "./FilesPane";
import { PrSplitButton } from "./PrSplitButton";
import { RefreshIcon } from "./icons";
import { TerminalPane } from "./TerminalPane";
import { ipc, newSessionId } from "../lib/ipc";
import { watchWorkspaceDirty } from "../lib/workspaceWatch";
import { openPullRequest } from "../lib/pullActions";
import { changedPaths, commitMessageFor, defaultBranchName, prControl, validBranchName, type PrItemId } from "../lib/changes";
import { DEFAULT_REPO_SETTINGS, baseBranchOf } from "../lib/repoSettings";
import {
  canStartReview,
  handoffSummary,
  loadReviewOptions,
  reviewContextFor,
  saveReviewOptions,
  type ReviewOptions,
} from "../lib/review";
import type { TranscriptItem } from "../lib/history";
import type { FileDiff, FileStatus, RepoGitSettings, RunRecord, WorktreeInfo } from "../types/orchestration";
import type { PullRequestStatus } from "../types/pulls";

type Mode = "committed" | "uncommitted";
type Tab = "files" | "changes";
type Props = {
  run: RunRecord;
  /** The workspace to open first. Without one, the run's first workspace opens. */
  initialPath?: string | null;
  transcript: TranscriptItem[];
  nameOf: (nodeId: string) => string;
  onClose: () => void;
};

const cacheKey = (path: string, mode: Mode) => `${path}|${mode}`;

/**
 * The review pane of a run, full width in the orchestration view, laid out like Conductor's: the
 * run's workspaces across the top, Create PR as a split button at the right, and two tabs, All
 * files and Changes. Changes shows the pull request diff and the uncommitted edits side by side
 * with their files. It follows the workspace while its agent works. A review can be started with
 * the context chosen in the drawer, and its terminal opens here too. Nothing is committed, pushed
 * or opened until the user presses a button.
 */
export function ChangesPanel({ run, initialPath = null, transcript, nameOf, onClose }: Props) {
  const [settings, setSettings] = useState<RepoGitSettings>(DEFAULT_REPO_SETTINGS);
  const base = baseBranchOf(settings);
  const [worktrees, setWorktrees] = useState<WorktreeInfo[]>([]);
  const [selectedPath, setSelectedPath] = useState(initialPath ?? "");
  const [tab, setTab] = useState<Tab>("changes");
  const [mode, setMode] = useState<Mode>("committed");
  // Until the user picks a diff, the panel opens on whichever one has something to show.
  const [modeChosen, setModeChosen] = useState(false);
  const [diffs, setDiffs] = useState<Record<string, FileDiff[]>>({});
  const [selectedFile, setSelectedFile] = useState<string | null>(null);
  const [pr, setPr] = useState<PullRequestStatus | null | undefined>(undefined);
  const [confirming, setConfirming] = useState(false);
  // The branch that work on the base branch moves to, editable in the confirmation.
  const [newBranch, setNewBranch] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState<string | null>(null);
  // Moves when the workspace's files may have changed, and when the user asks to read them again.
  const [version, setVersion] = useState(0);
  const [prVersion, setPrVersion] = useState(0);
  // Moves when a workspace may be on another branch, as after work was moved off the base branch.
  const [worktreesVersion, setWorktreesVersion] = useState(0);
  const [options, setOptions] = useState<ReviewOptions>(() => loadReviewOptions(run.repoPath));
  const [reviewOpen, setReviewOpen] = useState(false);
  const [review, setReview] = useState<{ sessionId: string; agent: string } | null>(null);
  const [reviewHidden, setReviewHidden] = useState(false);

  const branch = worktrees.find((item) => item.path === selectedPath)?.branch ?? null;
  const committed = diffs[cacheKey(selectedPath, "committed")];
  const uncommitted = diffs[cacheKey(selectedPath, "uncommitted")];
  const list = mode === "committed" ? committed : uncommitted;
  const file = list?.find((item) => item.path === selectedFile);
  const uncommittedCount = uncommitted?.length ?? 0;
  const committedCount = committed?.length ?? 0;
  const control = branch ? prControl({ branch, base, changedFiles: uncommittedCount, committedFiles: committedCount, pr }) : null;
  const onBaseBranch = branch !== null && branch === base;
  const changedCount = committed || uncommitted ? changedPaths(committed, uncommitted).length : null;
  // The status of each changed file, for the badges in All files. An edit made since the last commit wins.
  const changedStatus = useMemo(() => {
    const map = new Map<string, FileStatus>();
    for (const item of committed ?? []) map.set(item.path, item.status);
    for (const item of uncommitted ?? []) map.set(item.path, item.status);
    return map;
  }, [committed, uncommitted]);

  // The repository's settings and the run's workspaces. The base branch itself has no PR, so its
  // workspace is listed without one.
  useEffect(() => {
    let cancelled = false;
    Promise.all([ipc.getRepoGitSettings(run.repoPath).catch(() => DEFAULT_REPO_SETTINGS), ipc.runWorktrees(run.id)])
      .then(([loaded, listed]) => {
        if (cancelled) return;
        const withBranch = listed.filter((item) => item.branch);
        setSettings(loaded);
        setWorktrees(withBranch);
        // Opened without a workspace, it starts on the run's first.
        setSelectedPath((current) => current || withBranch[0]?.path || "");
      })
      .catch((reason: unknown) => {
        if (!cancelled) setError(String(reason));
      });
    return () => {
      cancelled = true;
    };
  }, [run.id, run.repoPath, worktreesVersion]);

  /** Reads both diffs of a workspace at once, so switching between them is instant. */
  const loadDiffs = useCallback(
    async (path: string) => {
      const [pullRequest, local] = await Promise.all([
        ipc.getBranchDiff(path, settings.baseRef).catch((reason: unknown) => {
          setError(String(reason));
          return [] as FileDiff[];
        }),
        ipc.getGitDiff(path).catch(() => [] as FileDiff[]),
      ]);
      setDiffs((current) => ({
        ...current,
        [cacheKey(path, "committed")]: pullRequest,
        [cacheKey(path, "uncommitted")]: local,
      }));
    },
    [settings.baseRef],
  );

  /** Reads the workspace again: its diffs, and its files if All files is open. */
  const reload = useCallback(() => {
    if (!selectedPath) return;
    void loadDiffs(selectedPath);
    setVersion((current) => current + 1);
  }, [selectedPath, loadDiffs]);
  const reloadRef = useRef(reload);
  reloadRef.current = reload;

  useEffect(() => {
    if (!selectedPath) return;
    void loadDiffs(selectedPath);
  }, [selectedPath, loadDiffs]);

  // Follow the workspace while its agent edits: the backend debounces filesystem bursts into one
  // dirty event. The watcher is shared with the toolbar's count, so each stops on its own.
  useEffect(() => {
    if (!selectedPath) return;
    return watchWorkspaceDirty(selectedPath, () => reloadRef.current());
  }, [selectedPath]);

  // An agent commits without touching a file, which the watcher cannot see, so a change in the
  // run's status, such as it finishing, reads the workspace again.
  const lastStatus = useRef(run.status);
  useEffect(() => {
    if (lastStatus.current === run.status) return;
    lastStatus.current = run.status;
    reloadRef.current();
  }, [run.status]);

  // The PR of the selected workspace. Its status is read when the workspace is picked, and again on request.
  useEffect(() => {
    setPr(undefined);
    if (!branch) return;
    let cancelled = false;
    ipc
      .pullRequestStatus(selectedPath, branch, settings.remote)
      .then((status) => {
        if (!cancelled) setPr(status);
      })
      .catch((reason: unknown) => {
        if (cancelled) return;
        setPr(undefined);
        // Without a status the PR button is hidden, so say why instead of leaving it missing.
        console.warn("[ChangesPanel] pull request status unavailable", { branch, reason });
        setError(`Could not read this branch's pull request: ${String(reason)}`);
      });
    return () => {
      cancelled = true;
    };
  }, [selectedPath, branch, settings.remote, prVersion]);

  // Open on the pull request diff, or on the uncommitted edits when the branch has no commits of its own yet.
  useEffect(() => {
    if (modeChosen || !committed || !uncommitted) return;
    setMode(committed.length === 0 && uncommitted.length > 0 ? "uncommitted" : "committed");
  }, [committed, uncommitted, modeChosen]);

  // Keep a file selected whenever the list changes, so the diff is never empty while files exist.
  useEffect(() => {
    setSelectedFile((current) => (list?.some((item) => item.path === current) ? current : (list?.[0]?.path ?? null)));
  }, [list]);

  const switchWorkspace = (path: string) => {
    setSelectedPath(path);
    setConfirming(false);
    setReview(null);
    setReviewHidden(false);
    setModeChosen(false);
    setError(null);
  };

  const chooseMode = (next: Mode) => {
    setMode(next);
    setModeChosen(true);
  };

  /** From All files: that file's diff, on whichever side holds it. */
  const showDiff = (path: string) => {
    const inUncommitted = uncommitted?.some((item) => item.path === path) ?? false;
    const inCommitted = committed?.some((item) => item.path === path) ?? false;
    chooseMode(inUncommitted && !inCommitted ? "uncommitted" : "committed");
    setSelectedFile(path);
    setTab("changes");
  };

  /** `startOn` moves work on the base branch onto that new branch first. */
  const createPr = async (commitFirst: boolean, startOn: string | null = null) => {
    if (!branch) return;
    setBusy(true);
    setError(null);
    try {
      const created = await openPullRequest({
        worktreePath: selectedPath,
        branch,
        base,
        remote: settings.remote,
        commitMessage: commitFirst ? commitMessageFor(run.goal) : null,
        newBranch: startOn,
      });
      setPr(created);
      setConfirming(false);
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
      reload();
      // The workspace may have changed branch, even when a later step failed.
      setWorktreesVersion((current) => current + 1);
    }
  };

  const copy = async (text: string, what: string) => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(`${what} copied`);
      window.setTimeout(() => setCopied(null), 1800);
    } catch (reason) {
      setError(`Could not copy: ${String(reason)}`);
    }
  };

  const onPrItem = (id: PrItemId) => {
    if (id === "create") void createPr(false);
    else if (id === "commit-and-create") setConfirming(true);
    else if (id === "branch-and-create") {
      setNewBranch(defaultBranchName(run.id));
      setConfirming(true);
    }
    else if (id === "copy-link" && pr) void copy(pr.url, "PR link");
    else if (id === "copy-branch" && branch) void copy(branch, "Branch name");
  };

  const updateOptions = (patch: Partial<ReviewOptions>) => {
    const next = { ...options, ...patch };
    setOptions(next);
    saveReviewOptions(run.repoPath, next);
  };

  const startReview = async () => {
    const sessionId = newSessionId();
    setError(null);
    try {
      const handoffs = handoffSummary(transcript, nameOf);
      await ipc.startReview(sessionId, selectedPath, settings.baseRef, options.agent, reviewContextFor(options, run.goal, handoffs));
      setReview({ sessionId, agent: options.agent });
      setReviewHidden(false);
      setReviewOpen(false);
    } catch (reason) {
      setError(String(reason));
    }
  };

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label="Changes"
      className="absolute inset-0 z-40 flex min-h-0 flex-col bg-zinc-950"
      onKeyDown={(event) => {
        // Escape closes the panel, except while a field or the review terminal has the keyboard.
        if (event.key === "Escape" && !isTyping(event.target)) onClose();
      }}
    >
      <header className="flex items-center gap-3 border-b border-zinc-800 bg-zinc-900/70 px-4 py-2.5">
        <div className="min-w-0 flex-1">
          <div className="truncate text-sm font-semibold text-zinc-100">Changes · {run.goal}</div>
          <div className="truncate text-[11px] text-zinc-500">
            Pull requests go to <span className="font-mono text-zinc-400">{base}</span> on{" "}
            <span className="font-mono text-zinc-400">{settings.remote}</span>
          </div>
        </div>
        <button
          type="button"
          onClick={onClose}
          className="rounded-md border border-zinc-700 px-2.5 py-1 text-xs text-zinc-200 hover:bg-zinc-800"
        >
          Close
        </button>
      </header>

      {worktrees.length > 1 ? (
        <div role="tablist" aria-label="Workspaces" className="flex gap-1 overflow-x-auto border-b border-zinc-800 px-3 pt-2">
          {worktrees.map((item) => {
            const selected = item.path === selectedPath;
            return (
              <button
                key={item.path}
                type="button"
                role="tab"
                aria-selected={selected}
                onClick={() => switchWorkspace(item.path)}
                className={`rounded-t-md border border-b-0 px-2.5 py-1 font-mono text-xs ${
                  selected ? "border-zinc-700 bg-zinc-900 text-zinc-100" : "border-transparent text-zinc-500 hover:text-zinc-300"
                }`}
              >
                {item.branch}
              </button>
            );
          })}
        </div>
      ) : null}

      <div className="flex flex-wrap items-center gap-2 border-b border-zinc-800 px-4 py-2">
        <div role="tablist" aria-label="Review" className="flex gap-1 text-xs">
          <PaneTab selected={tab === "files"} onSelect={() => setTab("files")}>
            All files
          </PaneTab>
          <PaneTab selected={tab === "changes"} onSelect={() => setTab("changes")}>
            Changes{changedCount ? <span className="ml-1.5 text-zinc-500">{changedCount}</span> : null}
          </PaneTab>
        </div>

        <div className="ml-auto flex flex-wrap items-center gap-2">
          {copied ? (
            <span role="status" className="text-[11px] text-emerald-300">
              {copied}
            </span>
          ) : null}
          <button
            type="button"
            aria-label="Refresh"
            title="Read the workspace and its PR again"
            onClick={() => {
              reload();
              setPrVersion((current) => current + 1);
            }}
            className="rounded-md border border-zinc-700 p-1.5 text-zinc-300 hover:bg-zinc-800"
          >
            <RefreshIcon />
          </button>
          {review && reviewHidden ? (
            <button
              type="button"
              onClick={() => setReviewHidden(false)}
              className="rounded-md border border-zinc-700 px-2.5 py-1 text-xs text-zinc-200 hover:bg-zinc-800"
            >
              Show review
            </button>
          ) : null}
          <button
            type="button"
            aria-expanded={reviewOpen}
            onClick={() => setReviewOpen((open) => !open)}
            className={`rounded-md border px-2.5 py-1 text-xs font-medium ${
              reviewOpen ? "border-zinc-500 bg-zinc-800 text-zinc-100" : "border-zinc-700 text-zinc-200 hover:bg-zinc-800"
            }`}
          >
            Review
          </button>
          {onBaseBranch && !control && committed && uncommitted ? (
            <span className="text-[11px] text-zinc-500" title="A PR needs a branch with its own commits">
              On {base}: nothing to open a PR for
            </span>
          ) : null}
          {control ? (
            <PrSplitButton control={control} busy={busy} busyLabel={confirming ? "Working…" : "Creating PR…"} onSelect={onPrItem} />
          ) : null}
        </div>
      </div>

      {error ? <div className="border-b border-red-500/30 bg-red-500/10 px-4 py-1.5 text-xs text-red-200">{error}</div> : null}

      {confirming && branch && onBaseBranch ? (
        <StartBranchConfirm
          base={base}
          remote={settings.remote}
          name={newBranch}
          onName={setNewBranch}
          committedCount={committedCount}
          uncommitted={uncommitted ?? []}
          message={commitMessageFor(run.goal)}
          busy={busy}
          onConfirm={() => void createPr(uncommittedCount > 0, newBranch.trim())}
          onCancel={() => setConfirming(false)}
        />
      ) : null}

      {confirming && branch && !onBaseBranch ? (
        <div className="space-y-2 border-b border-indigo-500/30 bg-indigo-500/5 px-4 py-3">
          <div className="text-xs text-zinc-200">
            Commit {uncommittedCount} file{uncommittedCount === 1 ? "" : "s"} on <span className="font-mono">{branch}</span>, then open the PR:
          </div>
          <ul className="max-h-24 space-y-0.5 overflow-y-auto font-mono text-[11px] text-zinc-400">
            {(uncommitted ?? []).map((item) => (
              <li key={item.path} className="truncate">
                {item.status} {item.path}
              </li>
            ))}
          </ul>
          <div className="truncate text-[11px] text-zinc-500" title={commitMessageFor(run.goal)}>
            Message: {commitMessageFor(run.goal)}
          </div>
          <div className="flex gap-2">
            <PrButton onClick={() => void createPr(true)} disabled={busy}>
              {busy ? "Working…" : `Commit ${uncommittedCount} file${uncommittedCount === 1 ? "" : "s"} & open PR`}
            </PrButton>
            <button
              type="button"
              onClick={() => setConfirming(false)}
              disabled={busy}
              className="rounded-md px-2.5 py-1 text-xs text-zinc-400 hover:bg-zinc-800 disabled:opacity-50"
            >
              Cancel
            </button>
          </div>
        </div>
      ) : null}

      {reviewOpen ? (
        <ReviewDrawer
          options={options}
          onChange={updateOptions}
          canStart={canStartReview(options) && !busy}
          onStart={() => void startReview()}
        />
      ) : null}

      {tab === "files" ? (
        selectedPath ? (
          <FilesPane worktreePath={selectedPath} changed={changedStatus} version={version} onShowDiff={showDiff} />
        ) : (
          <div className="p-6 text-sm text-zinc-500">{error ? "" : "Reading the run's workspaces…"}</div>
        )
      ) : (
        <>
          <div className="flex items-center gap-2 border-b border-zinc-800 px-4 py-1.5">
            <div role="radiogroup" aria-label="Diff" className="flex rounded-md border border-zinc-700 p-0.5 text-xs">
              <ModeButton selected={mode === "committed"} onSelect={() => chooseMode("committed")}>
                Pull request
              </ModeButton>
              <ModeButton selected={mode === "uncommitted"} onSelect={() => chooseMode("uncommitted")}>
                Uncommitted{uncommittedCount > 0 ? ` (${uncommittedCount})` : ""}
              </ModeButton>
            </div>
            <span className="text-[11px] text-zinc-500">
              {mode === "committed" ? `Commits on this branch since ${base}` : "Edits not committed yet"}
            </span>
          </div>
          <div className="flex min-h-0 flex-1">
            <div className="w-72 shrink-0 overflow-y-auto border-r border-zinc-800">
              {list === undefined ? (
                <div className="p-4 text-xs text-zinc-500">Reading changes…</div>
              ) : (
                <FileList files={list} selected={selectedFile} onSelect={setSelectedFile} />
              )}
            </div>
            <div className="min-w-0 flex-1 overflow-auto">
              <DiffDetail file={file} />
            </div>
          </div>
        </>
      )}

      {review && !reviewHidden ? (
        <div className="flex h-72 shrink-0 flex-col border-t border-zinc-800">
          <div className="flex items-center justify-between border-b border-zinc-800 px-3 py-1.5 text-xs text-zinc-400">
            <span>Review · {review.agent}</span>
            <button type="button" onClick={() => setReviewHidden(true)} className="text-zinc-500 hover:text-zinc-200">
              Hide
            </button>
          </div>
          <div className="min-h-0 flex-1">
            <TerminalPane sessionId={review.sessionId} interactive />
          </div>
        </div>
      ) : null}
    </div>
  );
}

/**
 * The confirmation for work done on the base branch. A PR cannot start from the base branch itself,
 * so the work moves to a new branch first. It lists each step and says what stays where it was.
 */
function StartBranchConfirm({
  base,
  remote,
  name,
  onName,
  committedCount,
  uncommitted,
  message,
  busy,
  onConfirm,
  onCancel,
}: {
  base: string;
  remote: string;
  name: string;
  onName: (name: string) => void;
  committedCount: number;
  uncommitted: FileDiff[];
  message: string;
  busy: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const valid = validBranchName(name.trim());
  const count = uncommitted.length;
  return (
    <div className="space-y-2.5 border-b border-indigo-500/30 bg-indigo-500/5 px-4 py-3">
      <div className="text-xs text-zinc-200">
        This work is on <span className="font-mono">{base}</span>, the branch PRs go into, so it needs a branch of its own first:
      </div>
      <ol className="list-inside list-decimal space-y-1.5 text-[11px] text-zinc-300">
        <li>
          <label className="inline-flex items-center gap-2">
            Create the branch
            <input
              value={name}
              onChange={(event) => onName(event.target.value)}
              aria-label="New branch name"
              aria-invalid={!valid}
              spellCheck={false}
              className={`w-64 rounded-md border bg-zinc-950 px-2 py-0.5 font-mono text-xs text-zinc-100 outline-none ${
                valid ? "border-zinc-700 focus:border-indigo-500" : "border-red-500/60"
              }`}
            />
          </label>{" "}
          from where {base} is now.{" "}
          {committedCount === 1 ? "1 file already committed here comes with it." : committedCount > 1 ? `${committedCount} files already committed here come with it.` : ""}
        </li>
        {count > 0 ? (
          <li>
            Commit the {count} uncommitted file{count === 1 ? "" : "s"}:
            <ul className="mt-1 max-h-20 space-y-0.5 overflow-y-auto pl-4 font-mono text-[10px] text-zinc-400">
              {uncommitted.map((item) => (
                <li key={item.path} className="list-none truncate">
                  {item.status} {item.path}
                </li>
              ))}
            </ul>
            <div className="truncate pl-4 text-[10px] text-zinc-500" title={message}>
              Message: {message}
            </div>
          </li>
        ) : null}
        <li>
          Push it to <span className="font-mono">{remote}</span> and open a PR into <span className="font-mono">{base}</span>.
        </li>
      </ol>
      <div className="text-[11px] text-zinc-500">
        Your local <span className="font-mono">{base}</span> is not changed, but this checkout moves onto the new branch.
      </div>
      <div className="flex gap-2">
        <PrButton onClick={onConfirm} disabled={busy || !valid}>
          {busy ? "Working…" : count > 0 ? "Create branch, commit & open PR" : "Create branch & open PR"}
        </PrButton>
        <button
          type="button"
          onClick={onCancel}
          disabled={busy}
          className="rounded-md px-2.5 py-1 text-xs text-zinc-400 hover:bg-zinc-800 disabled:opacity-50"
        >
          Cancel
        </button>
      </div>
    </div>
  );
}

function PaneTab({ selected, onSelect, children }: { selected: boolean; onSelect: () => void; children: ReactNode }) {
  return (
    <button
      type="button"
      role="tab"
      aria-selected={selected}
      onClick={onSelect}
      className={`rounded-md px-3 py-1.5 ${selected ? "bg-zinc-800 text-zinc-100" : "text-zinc-400 hover:text-zinc-200"}`}
    >
      {children}
    </button>
  );
}

function ModeButton({ selected, onSelect, children }: { selected: boolean; onSelect: () => void; children: ReactNode }) {
  return (
    <button
      type="button"
      role="radio"
      aria-checked={selected}
      onClick={onSelect}
      className={`rounded px-2.5 py-1 ${selected ? "bg-zinc-700 text-zinc-100" : "text-zinc-400 hover:text-zinc-200"}`}
    >
      {children}
    </button>
  );
}

function PrButton({ onClick, disabled, children }: { onClick: () => void; disabled: boolean; children: string }) {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      className="rounded-md bg-indigo-600 px-2.5 py-1 text-xs font-medium text-white hover:bg-indigo-500 disabled:opacity-50"
    >
      {children}
    </button>
  );
}

/** The review's context, chosen once and remembered for the repository. */
export function ReviewDrawer({
  options,
  onChange,
  canStart,
  onStart,
}: {
  options: ReviewOptions;
  onChange: (patch: Partial<ReviewOptions>) => void;
  canStart: boolean;
  onStart: () => void;
}) {
  return (
    <div className="space-y-3 border-b border-zinc-800 bg-zinc-900/60 px-4 py-3">
      <div className="flex flex-wrap gap-x-5 gap-y-2 text-xs text-zinc-300">
        <Check label="Committed changes" checked={options.committed} onChange={(value) => onChange({ committed: value })} />
        <Check label="Uncommitted edits" checked={options.uncommitted} onChange={(value) => onChange({ uncommitted: value })} />
        <Check label="Commit list" checked={options.commitLog} onChange={(value) => onChange({ commitLog: value })} />
        <Check label="Run goal" checked={options.includeGoal} onChange={(value) => onChange({ includeGoal: value })} />
        <Check label="Run handoffs" checked={options.includeHandoffs} onChange={(value) => onChange({ includeHandoffs: value })} />
      </div>
      <div className="grid gap-3 md:grid-cols-2">
        <label className="space-y-1 text-[11px] text-zinc-400">
          Reviewer
          <select
            value={options.agent}
            onChange={(event) => onChange({ agent: event.target.value === "codex" ? "codex" : "claude" })}
            className="block w-full rounded-md border border-zinc-700 bg-zinc-950 px-2 py-1 text-xs text-zinc-100"
          >
            <option value="claude">Claude</option>
            <option value="codex">Codex</option>
          </select>
        </label>
        <label className="space-y-1 text-[11px] text-zinc-400">
          Verify with
          <input
            value={options.testCommand}
            onChange={(event) => onChange({ testCommand: event.target.value })}
            placeholder="e.g. npm test"
            className="block w-full rounded-md border border-zinc-700 bg-zinc-950 px-2 py-1 font-mono text-xs text-zinc-100 placeholder:text-zinc-600"
          />
        </label>
      </div>
      <label className="block space-y-1 text-[11px] text-zinc-400">
        Focus
        <textarea
          rows={2}
          value={options.focus}
          onChange={(event) => onChange({ focus: event.target.value })}
          placeholder="What should the reviewer look hardest at?"
          className="block w-full resize-none rounded-md border border-zinc-700 bg-zinc-950 px-2 py-1 text-xs text-zinc-100 placeholder:text-zinc-600"
        />
      </label>
      <div className="flex items-center justify-between">
        <span className="text-[11px] text-zinc-500">Remembered for this repository.</span>
        <button
          type="button"
          onClick={onStart}
          disabled={!canStart}
          className="rounded-md bg-indigo-600 px-3 py-1.5 text-xs font-medium text-white hover:bg-indigo-500 disabled:opacity-50"
        >
          Start review
        </button>
      </div>
    </div>
  );
}

function Check({ label, checked, onChange }: { label: string; checked: boolean; onChange: (value: boolean) => void }) {
  return (
    <label className="flex cursor-pointer items-center gap-1.5">
      <input type="checkbox" checked={checked} onChange={(event) => onChange(event.target.checked)} className="accent-indigo-500" />
      {label}
    </label>
  );
}

function isTyping(target: EventTarget | null): boolean {
  return (
    target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement || target instanceof HTMLSelectElement
  );
}
