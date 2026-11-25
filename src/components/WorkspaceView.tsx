import { useCallback, useEffect, useState } from "react";
import { TerminalPane } from "./TerminalPane";
import { DiffDetail, FileList } from "./DiffViewer";
import { events, ipc, newSessionId } from "../lib/ipc";
import type { AgentProvider, FileDiff, WorktreeInfo } from "../types/orchestration";

type Notice = { tone: "info" | "error"; text: string };
type Session = { id: string; cwd: string; label: string };

const LAUNCHERS: Array<{ id: AgentProvider | "shell"; label: string }> = [
  { id: "shell", label: "Shell" },
  { id: "claude", label: "Claude" },
  { id: "codex", label: "Codex" },
  { id: "gemini", label: "Gemini" },
  { id: "opencode", label: "OpenCode" },
];

const FIELD =
  "w-full rounded-md border border-zinc-700 bg-zinc-950 px-2.5 py-1.5 text-xs text-zinc-100 outline-none placeholder:text-zinc-600 focus:border-indigo-500";

function basename(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
}

/** Worktree launcher, diff review, merge/remove, and a PTY terminal scoped to the workspace. */
export function WorkspaceView({ repoPath }: { repoPath: string }) {
  const [worktrees, setWorktrees] = useState<WorktreeInfo[]>([]);
  const [activePath, setActivePath] = useState<string | null>(null);
  const [files, setFiles] = useState<FileDiff[]>([]);
  const [selectedFile, setSelectedFile] = useState<string | null>(null);
  const [branchName, setBranchName] = useState("");
  const [folderName, setFolderName] = useState("");
  const [mergeTarget, setMergeTarget] = useState("main");
  const [squash, setSquash] = useState(true);
  const [notice, setNotice] = useState<Notice | null>(null);
  const [session, setSession] = useState<Session | null>(null);
  const [confirmRemove, setConfirmRemove] = useState(false);
  const [busy, setBusy] = useState(false);

  const refreshWorktrees = useCallback(async (): Promise<WorktreeInfo[]> => {
    try {
      const list = await ipc.listWorktrees(repoPath);
      setWorktrees(list);
      return list;
    } catch (error) {
      setNotice({ tone: "error", text: String(error) });
      return [];
    }
  }, [repoPath]);

  useEffect(() => {
    void refreshWorktrees().then((list) => {
      setActivePath((current) => current ?? list.find((wt) => wt.isMain)?.path ?? list[0]?.path ?? null);
    });
  }, [refreshWorktrees]);

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

  const createWorkspace = async () => {
    const branch = branchName.trim();
    if (!branch) return;
    const folder = folderName.trim() || branch.replace(/[^A-Za-z0-9._-]+/g, "-");
    setBusy(true);
    try {
      const info = await ipc.createWorktree(repoPath, branch, folder);
      setBranchName("");
      setFolderName("");
      await refreshWorktrees();
      setActivePath(info.path);
      setNotice({ tone: "info", text: `Workspace "${branch}" created in .worktrees/${folder}` });
    } catch (error) {
      setNotice({ tone: "error", text: String(error) });
    } finally {
      setBusy(false);
    }
  };

  const removeWorkspace = async (worktree: WorktreeInfo) => {
    if (!confirmRemove) {
      setConfirmRemove(true);
      return;
    }
    setBusy(true);
    try {
      await ipc.removeWorktree(repoPath, worktree.branch ?? "", basename(worktree.path));
      const list = await refreshWorktrees();
      setActivePath(list.find((wt) => wt.isMain)?.path ?? null);
      setNotice({ tone: "info", text: "Workspace removed. Its branch was kept unless it was fully merged." });
    } catch (error) {
      setNotice({ tone: "error", text: String(error) });
    } finally {
      setConfirmRemove(false);
      setBusy(false);
    }
  };

  const mergeWorkspace = async (worktree: WorktreeInfo) => {
    if (!worktree.branch) return;
    setBusy(true);
    try {
      const result = await ipc.mergeWorktree(repoPath, worktree.branch, mergeTarget.trim(), squash);
      const conflicts = result.conflicts.length > 0 ? ` Conflicts: ${result.conflicts.join(", ")}.` : "";
      setNotice({ tone: result.merged ? "info" : "error", text: `${result.message}${conflicts}` });
      if (result.merged) await refreshWorktrees();
    } catch (error) {
      setNotice({ tone: "error", text: String(error) });
    } finally {
      setBusy(false);
    }
  };

  const launch = async (provider: AgentProvider | "shell") => {
    if (!activePath) return;
    const id = newSessionId();
    const label = LAUNCHERS.find((item) => item.id === provider)?.label ?? provider;
    try {
      await ipc.spawnAgentPty(id, activePath, provider, 120, 30);
      setSession({ id, cwd: activePath, label });
    } catch (error) {
      setNotice({ tone: "error", text: String(error) });
    }
  };

  const sessionHere = session && session.cwd === activePath ? session : null;

  return (
    <div className="flex h-full min-h-0">
      <aside className="flex w-72 shrink-0 flex-col border-r border-zinc-800 bg-zinc-900/50">
        <div className="space-y-2 border-b border-zinc-800 p-3">
          <div className="text-[11px] font-semibold uppercase tracking-wider text-zinc-500">New workspace</div>
          <input
            value={branchName}
            onChange={(e) => setBranchName(e.target.value)}
            placeholder="branch, e.g. feature/dark-mode"
            className={FIELD}
            onKeyDown={(e) => e.key === "Enter" && void createWorkspace()}
          />
          <input
            value={folderName}
            onChange={(e) => setFolderName(e.target.value)}
            placeholder="folder (optional)"
            className={FIELD}
          />
          <button
            type="button"
            onClick={() => void createWorkspace()}
            disabled={busy || !branchName.trim()}
            className="w-full rounded-md bg-indigo-600 px-2.5 py-1.5 text-xs font-medium text-white hover:bg-indigo-500 disabled:cursor-not-allowed disabled:bg-zinc-800 disabled:text-zinc-500"
          >
            Create worktree
          </button>
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto p-2">
          <div className="px-2 pb-1.5 text-[11px] font-semibold uppercase tracking-wider text-zinc-500">Workspaces</div>
          <ul className="space-y-0.5">
            {worktrees.map((worktree) => {
              const active = worktree.path === activePath;
              return (
                <li key={worktree.path}>
                  <button
                    type="button"
                    onClick={() => setActivePath(worktree.path)}
                    className={`w-full rounded-md px-2.5 py-2 text-left ${active ? "bg-zinc-800" : "hover:bg-zinc-800/60"}`}
                  >
                    <div className="flex items-center gap-2">
                      <span className={`h-1.5 w-1.5 shrink-0 rounded-full ${worktree.isMain ? "bg-emerald-400" : "bg-indigo-400"}`} />
                      <span className="truncate text-xs font-medium text-zinc-100">
                        {worktree.branch ?? `detached ${worktree.head.slice(0, 8)}`}
                      </span>
                      {worktree.isMain ? <span className="ml-auto text-[10px] text-zinc-500">main</span> : null}
                    </div>
                    <div className="mt-0.5 truncate pl-3.5 font-mono text-[10px] text-zinc-500" title={worktree.path}>
                      {worktree.isMain ? worktree.path : `.worktrees/${basename(worktree.path)}`}
                    </div>
                  </button>
                </li>
              );
            })}
          </ul>
        </div>
      </aside>

      <section className="flex min-w-0 flex-1 flex-col">
        <div className="flex flex-wrap items-center gap-2 border-b border-zinc-800 bg-zinc-900/70 px-4 py-2.5">
          <div className="min-w-0">
            <div className="truncate text-sm font-semibold text-zinc-100">
              {activeWorktree?.branch ?? "No workspace selected"}
            </div>
            <div className="truncate font-mono text-[11px] text-zinc-500">{activePath}</div>
          </div>
          <div className="ml-auto flex flex-wrap items-center gap-2">
            {activeWorktree && !activeWorktree.isMain && activeWorktree.branch ? (
              <>
                <input
                  value={mergeTarget}
                  onChange={(e) => setMergeTarget(e.target.value)}
                  className={`${FIELD} w-28`}
                  aria-label="Merge target branch"
                />
                <label className="flex items-center gap-1 text-[11px] text-zinc-400">
                  <input type="checkbox" checked={squash} onChange={(e) => setSquash(e.target.checked)} className="accent-indigo-500" />
                  squash
                </label>
                <button
                  type="button"
                  onClick={() => void mergeWorkspace(activeWorktree)}
                  disabled={busy}
                  className="rounded-md border border-zinc-700 px-2.5 py-1.5 text-xs text-zinc-200 hover:bg-zinc-800 disabled:opacity-50"
                >
                  Merge
                </button>
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
