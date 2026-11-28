import { useCallback, useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { ipc } from "./lib/ipc";
import { useConnections } from "./lib/connections";
import { usePreferences } from "./lib/preferences";
import { canvasPaths, withoutCanvas } from "./lib/canvases";
import { isInside } from "./lib/archive";
import { GitHubConnection } from "./components/GitHubConnection";
import { ArchiveWorkspaceDialog } from "./components/ArchiveWorkspaceDialog";
import { OpenRepositoryDialog } from "./components/OpenRepositoryDialog";
import { OrchestrationCanvas } from "./components/OrchestrationCanvas";
import { RepoSettingsView } from "./components/RepoSettingsView";
import { SettingsView } from "./components/SettingsView";
import { Sidebar, type ViewMode } from "./components/Sidebar";
import { SplashScreen } from "./components/SplashScreen";
import { WorkspaceView, type Session } from "./components/WorkspaceView";
import { disposeTerminal } from "./lib/terminalStore";
import type { RepoMetadata, RepoRecord, WorktreeInfo } from "./types/orchestration";
import type { BranchPull } from "./types/pulls";

/** How often each repository's PRs and workspaces are read again, so merge states follow GitHub. */
const PULLS_REFRESH_MS = 60_000;

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
  // Every terminal session that is open, across workspaces, so switching workspace, opening Settings or
  // starting another terminal never hides one that is still running. Each workspace remembers the tab it showed.
  const [sessions, setSessions] = useState<Session[]>([]);
  const [selectedSessionIds, setSelectedSessionIds] = useState<Record<string, string>>({});
  // Each repository's PRs, by the branch they were opened from. Empty for a repository whose PRs
  // cannot be read, so its workspaces show no merge state.
  const [branchPulls, setBranchPulls] = useState<Record<string, BranchPull[]>>({});
  // A repository whose Git settings are open, and the repository a new workspace is being made in.
  const [repoSettingsFor, setRepoSettingsFor] = useState<string | null>(null);
  const [creating, setCreating] = useState<string | null>(null);
  // The workspace whose archive confirmation is open. Only the user starts this: a PR created or
  // merged never archives a workspace.
  const [archiving, setArchiving] = useState<{ repoPath: string; worktree: WorktreeInfo } | null>(null);
  // Repositories whose orchestration canvas is kept mounted, so a run's progress survives switching
  // views, opening settings or moving to another repository.
  const [canvasRepos, setCanvasRepos] = useState<string[]>([]);
  const connections = useConnections();
  const { preferences, update: updatePreferences } = usePreferences();
  const overlayOpen = settingsOpen || repoSettingsFor !== null;
  const orchestrating = mode === "orchestration";
  const canvases = canvasPaths(canvasRepos, { repoPath: repo?.path ?? null, orchestrating });

  // Remember a repository's canvas from the first time it opens.
  useEffect(() => {
    setCanvasRepos((kept) => {
      const next = canvasPaths(kept, { repoPath: repo?.path ?? null, orchestrating });
      return next.length === kept.length ? kept : next;
    });
  }, [repo?.path, orchestrating]);

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

  // A repository's PRs, read by one `gh` call. A repository without a GitHub remote, or without `gh`
  // signed in, keeps no list, and its workspaces show no merge state.
  const loadBranchPulls = useCallback(async (path: string) => {
    try {
      const list = await ipc.listPullRequests(path);
      setBranchPulls((current) => ({ ...current, [path]: list }));
    } catch {
      // Keep what was read before, if anything.
    }
  }, []);

  // Keyed by the paths, so opening a repository does not start another round of reads.
  const repoPaths = recent.map((item) => item.path).join("\n");
  useEffect(() => {
    const refresh = () => {
      for (const path of repoPaths.split("\n").filter(Boolean)) {
        // Worktrees come along, so a branch that moved since the last read compares against its new head.
        void loadWorktrees(path);
        void loadBranchPulls(path);
      }
    };
    refresh();
    const timer = window.setInterval(refresh, PULLS_REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [repoPaths, loadWorktrees, loadBranchPulls]);

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
      // An archived repository leaves the app, and so does its canvas and whatever it was running.
      if (value) setCanvasRepos((kept) => withoutCanvas(kept, path));
      await refreshLists();
    } catch (err) {
      setError(String(err));
    }
  };

  const removeRepo = async (path: string) => {
    try {
      await ipc.forgetRepository(path);
      if (repo?.path === path) setRepo(null);
      setCanvasRepos((kept) => withoutCanvas(kept, path));
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
      setRepoSettingsFor(null);
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
    setRepoSettingsFor(null);
  };

  /** "+" in the sidebar: a new workspace on a fresh branch from the repository's base, selected at once. */
  const createWorkspace = async (repoPath: string) => {
    if (creating) return;
    setCreating(repoPath);
    try {
      const info = await ipc.createWorkspace(repoPath);
      await loadWorktrees(repoPath);
      await selectWorktree(repoPath, info.path);
    } catch (err) {
      setError(String(err));
    } finally {
      setCreating(null);
    }
  };

  /**
   * A workspace was archived. Its repository's list is read again, the selection leaves it, and a
   * terminal that was running inside its folder is closed, since the folder is gone.
   */
  const finishArchive = async (repoPath: string, archived: WorktreeInfo) => {
    await loadWorktrees(repoPath);
    setActiveWorktreePath((current) => (current === archived.path ? null : current));
    endSessions(sessions.filter((item) => isInside(item.cwd, archived.path)).map((item) => item.id));
  };

  /** Ends terminal sessions: their processes are stopped and their tabs go. */
  const endSessions = (ids: string[]) => {
    if (ids.length === 0) return;
    const ended = new Set(ids);
    for (const id of ended) {
      void ipc.killAgentPty(id).catch(() => undefined);
      disposeTerminal(id);
    }
    setSessions((current) => current.filter((item) => !ended.has(item.id)));
    setSelectedSessionIds((current) =>
      Object.fromEntries(Object.entries(current).filter(([, id]) => !ended.has(id))),
    );
  };

  const startSession = (started: Session) => {
    setSessions((current) => [...current, started]);
    setSelectedSessionIds((current) => ({ ...current, [started.cwd]: started.id }));
  };

  const selectSession = (cwd: string, id: string) => {
    setSelectedSessionIds((current) => ({ ...current, [cwd]: id }));
  };

  /** The gear beside a repository: its Git settings, opening the repository first if needed. */
  const openRepoSettings = async (repoPath: string) => {
    if (repo?.path !== repoPath) {
      const failure = await openPath(repoPath);
      if (failure !== null) return;
    }
    setSettingsOpen(false);
    setRepoSettingsFor(repoPath);
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
          setRepoSettingsFor(null);
        }}
        activePath={repo?.path ?? null}
        activeWorktreePath={activeWorktreePath}
        recent={recent}
        archived={archived}
        worktrees={worktrees}
        branchPulls={branchPulls}
        onSelectWorktree={(repoPath, worktreePath) => void selectWorktree(repoPath, worktreePath)}
        onCreateWorkspace={(path) => void createWorkspace(path)}
        onArchiveWorkspace={(repoPath, worktree) => setArchiving({ repoPath, worktree })}
        onOpenRepoSettings={(path) => void openRepoSettings(path)}
        settingsActive={settingsOpen}
        onOpenSettings={() => {
          setRepoSettingsFor(null);
          setSettingsOpen(true);
        }}
        connections={connections}
        onArchive={(path, value) => void archiveRepo(path, value)}
        onRemove={(path) => void removeRepo(path)}
        onOpenRepository={() => setOpeningRepo(true)}
        preferences={preferences}
        onPreferencesChange={updatePreferences}
      />

      <div className="relative flex min-w-0 flex-1">
        {/* Kept mounted under Settings, hidden by visibility and not display, so it keeps its size:
            a terminal that is laid out at zero size would shrink its agent's PTY. */}
        <main
          className={`flex min-w-0 flex-1 flex-col ${overlayOpen ? "view-hidden" : ""}`}
          inert={overlayOpen}
          aria-hidden={overlayOpen || undefined}
        >
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

          <div className="relative min-h-0 flex-1">
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
            ) : mode === "workspaces" && !overlayOpen ? (
              <WorkspaceView
                key={repo.path}
                repoPath={repo.path}
                worktrees={worktrees[repo.path] ?? []}
                activePath={activeWorktreePath}
              onArchive={(worktree) => setArchiving({ repoPath: repo.path, worktree })}
              onArchived={(worktree) => void finishArchive(repo.path, worktree)}
                sessions={sessions}
                selectedSessionIds={selectedSessionIds}
                onSessionStart={startSession}
                onSessionSelect={selectSession}
                onSessionEnd={(id) => endSessions([id])}
                onPullChanged={() => void loadBranchPulls(repo.path)}
              />
            ) : null}
            {canvases.map((path) => {
              const shown = repo?.path === path && orchestrating && !overlayOpen;
              return (
                // One canvas per repository, mounted for good once opened. Hidden, it keeps its
                // layout and keeps listening to its run, but takes no clicks, focus or shortcuts.
                <div key={path} className={`absolute inset-0 bg-zinc-950 ${shown ? "" : "view-hidden"}`} inert={!shown} aria-hidden={!shown || undefined}>
                  <OrchestrationCanvas repoPath={path} active={shown} />
                </div>
              );
            })}
          </div>
        </main>
        {settingsOpen ? (
          <main className="absolute inset-0 min-w-0 bg-zinc-950">
            <SettingsView connections={connections} />
          </main>
        ) : repoSettingsFor ? (
          <main className="absolute inset-0 min-w-0 bg-zinc-950">
            <RepoSettingsView
              key={repoSettingsFor}
              repoPath={repoSettingsFor}
              repoName={recent.find((item) => item.path === repoSettingsFor)?.name ?? repoSettingsFor}
            />
          </main>
        ) : null}
      </div>

      {archiving ? (
        <ArchiveWorkspaceDialog
          key={archiving.worktree.path}
          repoPath={archiving.repoPath}
          worktree={archiving.worktree}
          onArchived={(worktree) => void finishArchive(archiving.repoPath, worktree)}
          onClose={() => setArchiving(null)}
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
