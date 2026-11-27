/**
 * Terminal sessions the workspaces view keeps open. A session outlives the view that shows it: switching
 * workspace, opening Settings or starting another terminal must not hide one that is still running.
 */

/** The sessions open in one workspace, in the order they were started. */
export function sessionsIn<T extends { cwd: string }>(sessions: readonly T[], cwd: string | null): T[] {
  if (cwd === null) return [];
  return sessions.filter((session) => session.cwd === cwd);
}

/**
 * The session a workspace's terminal shows: the one last picked for it, while it is still open there,
 * otherwise the workspace's most recently started session.
 */
export function shownSession<T extends { id: string; cwd: string }>(
  sessions: readonly T[],
  selectedId: string | undefined,
  cwd: string | null,
): T | null {
  const here = sessionsIn(sessions, cwd);
  return here.find((session) => session.id === selectedId) ?? here[here.length - 1] ?? null;
}
