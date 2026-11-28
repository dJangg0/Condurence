import type { RepoGitSettings } from "../types/orchestration";

/** The settings before anything is saved. Same values as the Rust defaults. */
export const DEFAULT_REPO_SETTINGS: RepoGitSettings = {
  baseRef: "origin/main",
  remote: "origin",
  deleteBranchOnArchive: false,
  archiveOnMerge: false,
  setUpstreamOnPush: true,
};

/** The base branch without its remote: `origin/release/1.0` becomes `release/1.0`. */
export function baseBranchOf(settings: RepoGitSettings): string {
  const slash = settings.baseRef.indexOf("/");
  return slash === -1 ? settings.baseRef : settings.baseRef.slice(slash + 1);
}
