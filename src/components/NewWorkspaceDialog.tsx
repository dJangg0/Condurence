import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { TerminalPane } from "./TerminalPane";
import { ipc, newSessionId } from "../lib/ipc";
import type { AgentProvider, WorktreeInfo } from "../types/orchestration";

export type WorkspaceLauncher = { id: AgentProvider | "shell"; label: string };

/** The terminals a workspace can start with. Shared by the new-workspace popup and the workspace view. */
export const LAUNCHERS: WorkspaceLauncher[] = [
  { id: "shell", label: "Shell" },
  { id: "claude", label: "Claude" },
  { id: "codex", label: "Codex" },
  { id: "gemini", label: "Gemini" },
  { id: "opencode", label: "OpenCode" },
];

export type DockedSession = { id: string; cwd: string; label: string };

const FIELD =
  "w-full rounded-md border border-zinc-700 bg-zinc-950 px-2.5 py-1.5 text-xs text-zinc-100 outline-none placeholder:text-zinc-600 focus:border-indigo-500";
const BUTTON_PRIMARY =
  "rounded-md bg-indigo-600 px-3 py-1.5 text-xs font-medium text-white hover:bg-indigo-500 disabled:cursor-not-allowed disabled:opacity-50";
const BUTTON_QUIET =
  "rounded-md border border-zinc-700 px-3 py-1.5 text-xs text-zinc-200 hover:bg-zinc-800 disabled:cursor-not-allowed disabled:opacity-50";

type Props = {
  repoPath: string;
  launchers?: WorkspaceLauncher[];
  /** Called as soon as the worktree exists, so the workspace list can refresh and select it. */
  onCreated: (worktree: WorktreeInfo) => void;
  /** Closes the popup. A running terminal is handed over to the workspace's bottom pane. */
  onClose: (docked: DockedSession | null) => void;
};

/**
 * The "+" popup. It creates a worktree for the next feature, then shows a floating terminal in
 * it with the chosen agent already started. Docking (or closing) moves the same running
 * terminal into the workspace view; nothing restarts.
 */
export function NewWorkspaceDialog({ repoPath, launchers = LAUNCHERS, onCreated, onClose }: Props) {
  const [branch, setBranch] = useState("");
  const [agent, setAgent] = useState<AgentProvider | "shell">(
    launchers.find((item) => item.id === "claude")?.id ?? launchers[0]?.id ?? "shell",
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Set once the worktree exists, so a failed terminal start can be retried without re-creating it.
  const [created, setCreated] = useState<WorktreeInfo | null>(null);
  const [terminal, setTerminal] = useState<DockedSession | null>(null);

  const label = launchers.find((item) => item.id === agent)?.label ?? agent;

  // Escape closes the form. Once the terminal is up it must reach the terminal, so it is not bound.
  useEffect(() => {
    if (terminal || busy) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose(null);
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [terminal, busy, onClose]);

  const start = async () => {
    const name = branch.trim();
    if (!name && !created) return;
    setBusy(true);
    setError(null);
    try {
      let worktree = created;
      if (!worktree) {
        const folder = name.replace(/[^A-Za-z0-9._-]+/g, "-");
        worktree = await ipc.createWorktree(repoPath, name, folder);
        setCreated(worktree);
        onCreated(worktree);
      }
      const id = newSessionId();
      try {
        await ipc.spawnAgentPty(id, worktree.path, agent, 120, 30);
      } catch (spawnError) {
        setError(`Workspace created, but the terminal could not start: ${String(spawnError)}`);
        return;
      }
      setTerminal({ id, cwd: worktree.path, label });
    } catch (createError) {
      setError(String(createError));
    } finally {
      setBusy(false);
    }
  };

  return createPortal(
    <div
      className="fixed inset-0 z-40 flex items-center justify-center bg-black/50 p-8"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget && !terminal && !busy) onClose(null);
      }}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-label="New workspace"
        className={`flex flex-col overflow-hidden rounded-lg border border-zinc-700 bg-zinc-900 shadow-2xl ${
          terminal ? "h-[min(520px,85vh)] w-[min(760px,100%)]" : "w-[min(440px,100%)]"
        }`}
      >
        <header className="flex items-center gap-3 border-b border-zinc-800 px-4 py-2.5">
          <div className="min-w-0 flex-1">
            <div className="truncate text-sm font-semibold text-zinc-100">
              {terminal ? (created?.branch ?? "New workspace") : "New workspace"}
            </div>
            <div className="truncate text-[11px] text-zinc-500">
              {terminal ? `${terminal.label} in .worktrees/${created?.path.split(/[\\/]/).pop()}` : "An isolated worktree for your next feature."}
            </div>
          </div>
          {terminal ? (
            <button
              type="button"
              onClick={() => onClose(terminal)}
              title="Close the popup and keep the terminal in the workspace"
              className={BUTTON_QUIET}
            >
              Dock
            </button>
          ) : null}
        </header>

        {terminal ? (
          <div className="min-h-0 flex-1">
            <TerminalPane sessionId={terminal.id} interactive />
          </div>
        ) : (
          <div className="space-y-3 p-4">
            <label className="block space-y-1.5">
              <span className="text-[11px] font-semibold uppercase tracking-wider text-zinc-500">Branch</span>
              <input
                value={created?.branch ?? branch}
                onChange={(event) => setBranch(event.target.value)}
                onKeyDown={(event) => event.key === "Enter" && void start()}
                placeholder="feature/dark-mode"
                disabled={busy || created !== null}
                autoFocus
                className={FIELD}
              />
            </label>

            <div className="space-y-1.5">
              <span className="text-[11px] font-semibold uppercase tracking-wider text-zinc-500">Start with</span>
              <div role="radiogroup" aria-label="Start with" className="flex flex-wrap gap-1.5">
                {launchers.map((launcher) => (
                  <button
                    key={launcher.id}
                    type="button"
                    role="radio"
                    aria-checked={agent === launcher.id}
                    onClick={() => setAgent(launcher.id)}
                    disabled={busy}
                    className={`rounded-md px-2.5 py-1.5 text-xs ${
                      agent === launcher.id
                        ? "bg-zinc-700 text-zinc-100"
                        : "border border-zinc-700 text-zinc-400 hover:bg-zinc-800"
                    }`}
                  >
                    {launcher.label}
                  </button>
                ))}
              </div>
            </div>

            {error ? (
              <div className="rounded-md border border-red-500/30 bg-red-500/10 px-2 py-1.5 text-[11px] text-red-200">
                {error}
              </div>
            ) : null}

            <div className="flex items-center justify-end gap-2 pt-1">
              <button type="button" onClick={() => onClose(null)} disabled={busy} className={BUTTON_QUIET}>
                {created ? "Close" : "Cancel"}
              </button>
              <button
                type="button"
                onClick={() => void start()}
                disabled={busy || (!created && !branch.trim())}
                className={BUTTON_PRIMARY}
              >
                {busy ? "Starting…" : created ? "Open terminal" : `Create and start ${label}`}
              </button>
            </div>
          </div>
        )}
      </div>
    </div>,
    document.body,
  );
}
