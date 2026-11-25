import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { ipc } from "./lib/ipc";
import { OrchestrationCanvas } from "./components/OrchestrationCanvas";
import { WorkspaceView } from "./components/WorkspaceView";
import type { RepoMetadata, RepoRecord } from "./types/orchestration";

type Mode = "workspaces" | "orchestration";

const MODES: Array<{ id: Mode; label: string; hint: string }> = [
  { id: "workspaces", label: "Workspaces", hint: "Isolated worktrees, diffs and terminals" },
  { id: "orchestration", label: "Agent Orchestration", hint: "Railway-style multi-agent topology" },
];

/** Dual-mode ADE shell: a repository sidebar plus the Workspaces or Orchestration surface. */
export default function App() {
  const [mode, setMode] = useState<Mode>("workspaces");
  const [repo, setRepo] = useState<RepoMetadata | null>(null);
  const [recent, setRecent] = useState<RepoRecord[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    ipc
      .listRecentRepositories()
      .then(setRecent)
      .catch(() => undefined);
  }, []);

  const openPath = async (path: string) => {
    try {
      const metadata = await ipc.openRepository(path);
      setRepo(metadata);
      setError(null);
      setRecent(await ipc.listRecentRepositories());
    } catch (err) {
      setError(String(err));
    }
  };

  const pickRepository = async () => {
    const selected = await open({ directory: true, multiple: false, title: "Choose a git repository" });
    if (typeof selected === "string") await openPath(selected);
  };

  return (
    <div className="flex h-full bg-zinc-950 text-zinc-100">
      <aside className="flex w-60 shrink-0 flex-col border-r border-zinc-800 bg-zinc-900/60">
        <div className="border-b border-zinc-800 px-4 py-3.5">
          <div className="text-sm font-semibold tracking-tight">OpenConductor</div>
          <div className="text-[11px] text-zinc-500">Agentic development environment</div>
        </div>
        <div className="p-3">
          <button
            type="button"
            onClick={() => void pickRepository()}
            className="w-full rounded-md bg-zinc-800 px-3 py-2 text-xs font-medium text-zinc-100 hover:bg-zinc-700"
          >
            Open repository…
          </button>
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto px-2 pb-3">
          <div className="px-2 pb-1.5 text-[11px] font-semibold uppercase tracking-wider text-zinc-500">Recent</div>
          {recent.length === 0 ? <div className="px-2 text-xs text-zinc-600">No repositories yet.</div> : null}
          <ul className="space-y-0.5">
            {recent.map((item) => (
              <li key={item.path}>
                <button
                  type="button"
                  onClick={() => void openPath(item.path)}
                  className={`w-full truncate rounded-md px-2 py-1.5 text-left text-xs ${
                    repo?.path === item.path ? "bg-zinc-800 text-zinc-100" : "text-zinc-400 hover:bg-zinc-800/60"
                  }`}
                  title={item.path}
                >
                  {item.name}
                </button>
              </li>
            ))}
          </ul>
        </div>
      </aside>

      <main className="flex min-w-0 flex-1 flex-col">
        <header className="flex items-center gap-4 border-b border-zinc-800 bg-zinc-900/40 px-4 py-2">
          <div className="min-w-0">
            <div className="truncate text-sm font-medium">{repo?.name ?? "No repository open"}</div>
            {repo ? (
              <div className="truncate text-[11px] text-zinc-500">
                on <span className="font-mono text-zinc-400">{repo.currentBranch}</span>
                {repo.hasGraph ? "" : " · no topology yet"}
              </div>
            ) : null}
          </div>
          <nav className="ml-auto flex rounded-lg border border-zinc-800 bg-zinc-950 p-0.5" aria-label="Views">
            {MODES.map((item) => (
              <button
                key={item.id}
                type="button"
                title={item.hint}
                onClick={() => setMode(item.id)}
                className={`rounded-md px-3 py-1.5 text-xs font-medium transition-colors ${
                  mode === item.id ? "bg-zinc-800 text-zinc-100 shadow" : "text-zinc-500 hover:text-zinc-300"
                }`}
              >
                {item.label}
              </button>
            ))}
          </nav>
        </header>

        {error ? (
          <div className="border-b border-red-500/30 bg-red-500/10 px-4 py-2 text-xs text-red-200">{error}</div>
        ) : null}

        <div className="min-h-0 flex-1">
          {!repo ? (
            <div className="flex h-full flex-col items-center justify-center gap-3 text-center">
              <div className="text-lg font-semibold text-zinc-200">Open a git repository to begin</div>
              <p className="max-w-md text-sm text-zinc-500">
                Work in parallel isolated worktrees with live diffs and terminals, or wire agents into a railway-style
                orchestration graph.
              </p>
              <button
                type="button"
                onClick={() => void pickRepository()}
                className="rounded-md bg-indigo-600 px-4 py-2 text-sm font-medium text-white hover:bg-indigo-500"
              >
                Open repository…
              </button>
            </div>
          ) : mode === "workspaces" ? (
            <WorkspaceView key={repo.path} repoPath={repo.path} />
          ) : (
            <OrchestrationCanvas key={repo.path} repoPath={repo.path} />
          )}
        </div>
      </main>
    </div>
  );
}
