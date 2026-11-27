/**
 * Which repositories keep an orchestration canvas mounted. A canvas holds its run's progress, the
 * goal being typed and unsaved topology edits as component state, so it must outlive the view
 * being switched away from. It is created the first time Agent Orchestration opens for a
 * repository, and kept, hidden, until the repository is archived or removed.
 */
export function canvasPaths(
  kept: readonly string[],
  current: { repoPath: string | null; orchestrating: boolean },
): string[] {
  if (current.orchestrating && current.repoPath && !kept.includes(current.repoPath)) {
    return [...kept, current.repoPath];
  }
  return [...kept];
}

/** The same list without a repository that is no longer open to the user. */
export function withoutCanvas(kept: readonly string[], repoPath: string): string[] {
  return kept.filter((path) => path !== repoPath);
}
