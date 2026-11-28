import { useEffect, useState } from "react";
import { ipc } from "../lib/ipc";
import { archiveResult, archiveSummary, workspaceFolder } from "../lib/archive";
import type { ArchivePreview, RepoGitSettings, WorktreeInfo } from "../types/orchestration";

const BUTTON_QUIET =
  "rounded-md border border-zinc-700 px-2.5 py-1.5 text-xs text-zinc-200 hover:bg-zinc-800 disabled:cursor-not-allowed disabled:opacity-50";

type Phase = "loading" | "ready" | "working" | "done";

/** The most files listed before the rest are counted. */
const FILES_SHOWN = 8;

/**
 * Archiving one temporary workspace, always on the user's own request. A workspace stays where it
 * is after its PR is created or merged: nothing archives it but this dialog. It reads what would
 * be discarded and kept before anything happens, and uncommitted changes are only discarded when
 * the button says so.
 */
export function ArchiveWorkspaceDialog({
  repoPath,
  worktree,
  onArchived,
  onClose,
}: {
  repoPath: string;
  worktree: WorktreeInfo;
  /** Called once the workspace is gone, so the lists and the selection can follow. */
  onArchived: (worktree: WorktreeInfo) => void;
  onClose: () => void;
}) {
  const [phase, setPhase] = useState<Phase>("loading");
  const [preview, setPreview] = useState<ArchivePreview | null>(null);
  const [settings, setSettings] = useState<RepoGitSettings | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<string | null>(null);
  const branch = worktree.branch;
  const label = branch ?? `detached ${worktree.head.slice(0, 8)}`;

  useEffect(() => {
    let cancelled = false;
    Promise.all([ipc.archivePreview(repoPath, worktree.path, branch), ipc.getRepoGitSettings(repoPath)])
      .then(([read, loaded]) => {
        if (cancelled) return;
        setPreview(read);
        setSettings(loaded);
        setPhase("ready");
      })
      .catch((reason: unknown) => {
        if (cancelled) return;
        setError(String(reason));
        setPhase("ready");
      });
    return () => {
      cancelled = true;
    };
  }, [repoPath, worktree.path, branch]);

  const summary = preview && settings ? archiveSummary(preview, { branch, deleteBranch: settings.deleteBranchOnArchive }) : null;

  const confirm = async () => {
    if (!summary) return;
    setPhase("working");
    setError(null);
    try {
      const outcome = await ipc.archiveWorkspace(repoPath, branch, workspaceFolder(worktree.path), summary.discards.length > 0);
      setResult(archiveResult(outcome, branch));
      setPhase("done");
      onArchived(worktree);
    } catch (reason) {
      setError(String(reason));
      setPhase("ready");
    }
  };

  const busy = phase === "working";
  return (
    <div
      className="fixed inset-0 z-40 flex items-center justify-center bg-black/60 p-8"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget && !busy) onClose();
      }}
      onKeyDown={(event) => {
        if (event.key === "Escape" && !busy) onClose();
      }}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-label={`Archive ${label}`}
        className="flex max-h-[min(560px,80vh)] w-[min(480px,100%)] flex-col overflow-hidden rounded-lg border border-zinc-700 bg-zinc-900 shadow-2xl"
      >
        <header className="border-b border-zinc-800 px-4 py-3">
          <div className="text-sm font-semibold text-zinc-100">Archive {label}</div>
          <div className="text-[11px] text-zinc-500">Removes this workspace's folder. Creating or merging its PR never does that for you.</div>
        </header>

        <div className="min-h-0 flex-1 space-y-3 overflow-y-auto p-4 text-xs text-zinc-300">
          {phase === "loading" ? <p className="text-zinc-500">Checking the workspace…</p> : null}

          {phase === "done" && result ? <p role="status" className="text-emerald-300">{result}</p> : null}

          {summary && phase !== "done" ? (
            <>
              {summary.discards.length > 0 ? (
                <div className="rounded-md border border-red-500/40 bg-red-500/10 p-2.5">
                  <div className="font-medium text-red-200">
                    {summary.discards.length} uncommitted file{summary.discards.length === 1 ? "" : "s"} will be discarded:
                  </div>
                  <ul className="mt-1.5 space-y-0.5 font-mono text-[11px] text-red-100/90">
                    {summary.discards.slice(0, FILES_SHOWN).map((file) => (
                      <li key={file} className="truncate" title={file}>
                        {file}
                      </li>
                    ))}
                  </ul>
                  {summary.discards.length > FILES_SHOWN ? (
                    <div className="mt-1 text-[11px] text-red-200/80">and {summary.discards.length - FILES_SHOWN} more</div>
                  ) : null}
                </div>
              ) : null}
              {summary.unpushed ? (
                <p className="rounded-md border border-amber-500/30 bg-amber-500/10 p-2.5 text-amber-100">{summary.unpushed}</p>
              ) : null}
              <ul className="space-y-1.5 text-zinc-300">
                <li>{summary.pr}</li>
                <li>{summary.branch}</li>
              </ul>
            </>
          ) : null}

          {error ? <p role="alert" className="rounded-md border border-red-500/30 bg-red-500/10 p-2.5 text-red-200">{error}</p> : null}
        </div>

        <footer className="flex justify-end gap-2 border-t border-zinc-800 px-4 py-3">
          {phase === "done" ? (
            <button type="button" onClick={onClose} autoFocus className={BUTTON_QUIET}>
              Done
            </button>
          ) : (
            <>
              <button type="button" onClick={onClose} disabled={busy} className={BUTTON_QUIET}>
                Cancel
              </button>
              <button
                type="button"
                onClick={() => void confirm()}
                disabled={!summary || busy}
                autoFocus
                className={`rounded-md px-2.5 py-1.5 text-xs font-medium text-white disabled:cursor-not-allowed disabled:opacity-50 ${
                  summary && summary.discards.length > 0 ? "bg-red-600 hover:bg-red-500" : "bg-indigo-600 hover:bg-indigo-500"
                }`}
              >
                {busy ? "Archiving…" : (summary?.confirmLabel ?? "Archive")}
              </button>
            </>
          )}
        </footer>
      </div>
    </div>
  );
}
