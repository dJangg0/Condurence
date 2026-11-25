import { useCallback, useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { ipc } from "./lib/ipc";
import { useConnections } from "./lib/connections";
import { usePreferences } from "./lib/preferences";
import { GitHubConnection } from "./components/GitHubConnection";
import { NewWorkspaceDialog, type DockedSession } from "./components/NewWorkspaceDialog";
import { OpenRepositoryDialog } from "./components/OpenRepositoryDialog";
import { OrchestrationCanvas } from "./components/OrchestrationCanvas";
import { SettingsView } from "./components/SettingsView";
import { Sidebar, type ViewMode } from "./components/Sidebar";
import { SplashScreen } from "./components/SplashScreen";
import { WorkspaceView, type Session } from "./components/WorkspaceView";
import type { RepoMetadata, RepoRecord, WorktreeInfo } from "./types/orchestration";

/** Dual-mode ADE shell: a sidebar with the views, repositories and settings, plus the Workspaces or Orchestration surface. */
export default function App() {
  const [mode, setMode] = useState<ViewMode>("workspaces");
  const [repo, setRepo] = useState<RepoMetadata | null>(null);
  const [recent, setRecent] = useState<RepoRecord[]>([]);
  const [archived, setArchived] = useState<RepoRecord[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [openingRepo, setOpeningRepo] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  // Workspaces per repository, for the sidebar groups.
  const [worktrees, setWorktrees] = useState<Record<string, WorktreeInfo[]>>({});
  const requested = useRef(new Set<string>());
  // The selected workspace of the open repository, or `null` for its main checkout.
  const [activeWorktreePath, setActiveWorktreePath] = useState<string | null>(null);
  const [session, setSession] = useState<Session | null>(null);
  const [creatingFor, setCreatingFor] = useState<string | null>(null);
  const connections = useConnections();
  const { preferences, update: updatePreferences } = usePreferences();

  const loadWorktrees = useCallback(async (path: string): Promise<WorktreeInfo[]> => {
    try {
      const list = await ipc.listWorktrees(path);
      setWorktrees((current) => ({ ...current, [path]: list }));
      return list;
    } catch {
      // A repository that moved or was deleted shows an empty group instead of failing the sidebar.
      setWorktrees((current) => ({ ...current, [path]: [] }));
      return [];
    }
  }, []);

  // Load each repository's workspaces once, the first time it appears in the list.
  useEffect(() => {
    for (const item of recent) {
      if (requested.current.has(item.path)) continue;
      requested.current.add(item.path);
      void loadWorktrees(item.path);
    }
  }, [recent, loadWorktrees]);

  const refreshLists = async () => {
    const [next, past] = await Promise.all([ipc.listRecentRepositories(), ipc.listArchivedRepositories()]);
    setRecent(next);
    setArchived(past);
  };

  useEffect(() => {
    void refreshLists().catch(() => undefined);
  }, []);

  const archiveRepo = async (path: string, value: boolean) => {
    try {
      await ipc.archiveRepository(path, value);
      if (value && repo?.path === path) setRepo(null);
      await refreshLists();
    } catch (err) {
      setError(String(err));
    }
  };

  const removeRepo = async (path: string) => {
    try {
      await ipc.forgetRepository(path);
      if (repo?.path === path) setRepo(null);
      await refreshLists();
    } catch (err) {
      setError(String(err));
    }
  };

  /** Opens a checkout. Resolves to an error message, or `null` when the repository opened. */
  const openPath = async (path: string): Promise<string | null> => {
    try {
      const metadata = await ipc.openRepository(path);
      setRepo(metadata);
      setActiveWorktreePath(null);
      setSettingsOpen(false);
      setError(null);
      await refreshLists();
      void loadWorktrees(metadata.path);
      return null;
    } catch (err) {
      setError(String(err));
      return String(err);
    }
  };

  /** Selects a workspace in the sidebar, opening its repository first when another one is open. */
  const selectWorktree = async (repoPath: string, worktreePath: string) => {
    if (repo?.path !== repoPath) {
      const failure = await openPath(repoPath);
      if (failure !== null) return;
    }
    setActiveWorktreePath(worktreePath);
    setSettingsOpen(false);
  };

  const workspaceCreated = async (repoPath: string, info: WorktreeInfo) => {
    await loadWorktrees(repoPath);
    await selectWorktree(repoPath, info.path);
  };

  const closeNewWorkspace = (docked: DockedSession | null) => {
    setCreatingFor(null);
    if (docked) setSession(docked);
  };

  const pickRepository = async () => {
    const selected = await open({ directory: true, multiple: false, title: "Choose a git repository" });
    if (typeof selected === "string") await openPath(selected);
  };

  return (
    <div className="flex h-full bg-zinc-950 text-zinc-100">
      <Sidebar
        mode={mode}
        onModeChange={(next) => {
          setMode(next);
          setSettingsOpen(false);
        }}
        activePath={repo?.path ?? null}
        activeWorktreePath={activeWorktreePath}
        recent={recent}
        archived={archived}
        worktrees={worktrees}
        onSelectWorktree={(repoPath, worktreePath) => void selectWorktree(repoPath, worktreePath)}
        onCreateWorkspace={setCreatingFor}
        settingsActive={settingsOpen}
        onOpenSettings={() => setSettingsOpen(true)}
        connections={connections}
        onArchive={(path, value) => void archiveRepo(path, value)}
        onRemove={(path) => void removeRepo(path)}
        onOpenRepository={() => setOpeningRepo(true)}
        preferences={preferences}
        onPreferencesChange={updatePreferences}
      />

      {settingsOpen ? (
        <main className="min-w-0 flex-1">
          <SettingsView connections={connections} />
        </main>
      ) : (
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
          <GitHubConnection repoPath={repo?.path ?? null} />
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
                onClick={() => setOpeningRepo(true)}
                className="rounded-md bg-indigo-600 px-4 py-2 text-sm font-medium text-white hover:bg-indigo-500"
              >
                Open repository…
              </button>
            </div>
          ) : mode === "workspaces" ? (
            <WorkspaceView
              key={repo.path}
              repoPath={repo.path}
              worktrees={worktrees[repo.path] ?? []}
              activePath={activeWorktreePath}
              onSelect={setActiveWorktreePath}
              onWorktreesChanged={() => loadWorktrees(repo.path)}
              session={session}
              onSessionChange={setSession}
            />
          ) : (
            <OrchestrationCanvas key={repo.path} repoPath={repo.path} />
          )}
        </div>
      </main>
      )}

      {creatingFor ? (
        <NewWorkspaceDialog
          repoPath={creatingFor}
          onCreated={(info) => void workspaceCreated(creatingFor, info)}
          onClose={closeNewWorkspace}
        />
      ) : null}

      {openingRepo ? (
        <OpenRepositoryDialog
          onOpenPath={openPath}
          onChooseLocal={() => {
            setOpeningRepo(false);
            void pickRepository();
          }}
          onClose={() => setOpeningRepo(false)}
        />
      ) : null}

      <SplashScreen />
    </div>
  );
}
