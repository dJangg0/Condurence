/**
 * GitHub connection contracts. Mirrors `src-tauri/src/github/mod.rs`, which serializes camelCase.
 * Condurence never sees a GitHub token; these describe state read through the `gh` CLI.
 */

export type GitHubAccount = {
  /** False when the `gh` binary cannot be found. */
  cliInstalled: boolean;
  signedIn: boolean;
  login: string | null;
  name: string | null;
};

/**
 * - `linked`: origin is a GitHub remote and `gh` confirmed access.
 * - `unverified`: origin is a GitHub remote that `gh` could not confirm. See `detail`.
 * - `unlinked`: the repository has no origin remote.
 * - `otherRemote`: origin points at a host other than github.com.
 */
export type GitHubLinkState = "linked" | "unverified" | "unlinked" | "otherRemote";

export type GitHubRepoConnection = {
  state: GitHubLinkState;
  /** The origin URL with credentials removed. */
  remoteUrl: string | null;
  nameWithOwner: string | null;
  webUrl: string | null;
  isPrivate: boolean | null;
  viewerPermission: string | null;
  defaultBranch: string | null;
  detail: string | null;
};

/** A repository from the signed-in account, for the connect picker. */
export type GitHubRepoSummary = {
  nameWithOwner: string;
  url: string;
  isPrivate: boolean;
  description: string | null;
};
