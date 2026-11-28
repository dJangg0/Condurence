/** Pull request contracts. Mirrors `src-tauri/src/pulls/mod.rs`, which serializes camelCase. */

export type ChecksState = "none" | "pending" | "passing" | "failing";

export type PullRequestStatus = {
  number: number;
  url: string;
  /** GitHub's state: `OPEN`, `CLOSED` or `MERGED`. */
  state: string;
  isDraft: boolean;
  /** `MERGEABLE`, `CONFLICTING`, or `UNKNOWN` while GitHub computes it. */
  mergeable: string;
  mergeStateStatus: string;
  reviewDecision: string | null;
  checks: ChecksState;
  /** The commit at the tip of the PR's branch as GitHub last saw it. */
  headSha?: string | null;
};

/** A PR from the repository's list, named by the branch it was opened from. Mirrors `BranchPull`. */
export type BranchPull = {
  branch: string;
  number: number;
  url: string;
  /** GitHub's state: `OPEN`, `CLOSED` or `MERGED`. */
  state: string;
  headSha: string | null;
};

/** What `sync_with_base` did. `conflicts` lists files still unmerged when it stopped. */
export type SyncOutcome = {
  pushed: boolean;
  conflicts: string[];
  message: string;
};
