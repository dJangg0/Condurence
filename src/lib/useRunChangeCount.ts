import { useCallback, useEffect, useRef, useState } from "react";
import { ipc } from "./ipc";
import { runChangeCount, type RunChangeCount } from "./changes";
import { DEFAULT_REPO_SETTINGS } from "./repoSettings";
import { watchWorkspaceDirty } from "./workspaceWatch";
import type { FileDiff } from "../types/orchestration";

/** The count is read at most this often, so a busy agent does not make a `git diff` per keystroke. */
const MIN_INTERVAL_MS = 2000;

/**
 * How much a run has changed, as lines added and removed, for the toolbar. It is read once the run is known and again
 * whenever one of the run's workspaces changes on disk or `refreshKey` moves, such as when the run
 * journals an event or the review pane closes. `null` until the first read, and when there is no run.
 */
export function useRunChangeCount(repoPath: string, runId: string | null, refreshKey: unknown): RunChangeCount | null {
  const [count, setCount] = useState<RunChangeCount | null>(null);
  const [paths, setPaths] = useState<string[]>([]);
  // A read for a run that is no longer shown is dropped.
  const current = useRef({ repoPath, runId });
  current.current = { repoPath, runId };

  const load = useCallback(async () => {
    if (!runId) return;
    const asked = { repoPath, runId };
    try {
      const [settings, worktrees] = await Promise.all([
        ipc.getRepoGitSettings(repoPath).catch(() => DEFAULT_REPO_SETTINGS),
        ipc.runWorktrees(runId),
      ]);
      const owned = worktrees.filter((worktree) => worktree.branch);
      const workspaces = await Promise.all(
        owned.map(async (worktree) => {
          const [committed, uncommitted] = await Promise.all([
            ipc.getBranchDiff(worktree.path, settings.baseRef).catch(() => [] as FileDiff[]),
            ipc.getGitDiff(worktree.path).catch(() => [] as FileDiff[]),
          ]);
          return { committed, uncommitted };
        }),
      );
      if (current.current.repoPath !== asked.repoPath || current.current.runId !== asked.runId) return;
      setCount(runChangeCount(workspaces));
      const next = owned.map((worktree) => worktree.path);
      setPaths((previous) => (previous.join("\n") === next.join("\n") ? previous : next));
    } catch {
      // The count is a hint. When it cannot be read, the last one stays.
    }
  }, [repoPath, runId]);

  // Reads are spaced out: one waits for the earlier one's interval to pass, and the rest join it.
  const loadRef = useRef(load);
  loadRef.current = load;
  const lastRead = useRef(0);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const request = useCallback(() => {
    if (timer.current !== undefined) return;
    const wait = Math.max(0, lastRead.current + MIN_INTERVAL_MS - Date.now());
    timer.current = setTimeout(() => {
      timer.current = undefined;
      lastRead.current = Date.now();
      void loadRef.current();
    }, wait);
  }, []);

  // A different run starts from nothing.
  useEffect(() => {
    setCount(null);
    setPaths([]);
  }, [repoPath, runId]);

  useEffect(() => {
    if (runId) request();
  }, [runId, repoPath, refreshKey, request]);

  // Files changing under any workspace of the run read the count again.
  const pathsKey = paths.join("\n");
  useEffect(() => {
    const stops = paths.map((path) => watchWorkspaceDirty(path, request));
    return () => stops.forEach((stop) => stop());
    // `pathsKey` stands for `paths`, which is a new array on every read.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pathsKey, request]);

  useEffect(
    () => () => {
      if (timer.current !== undefined) clearTimeout(timer.current);
      timer.current = undefined;
    },
    [],
  );

  return runId ? count : null;
}
