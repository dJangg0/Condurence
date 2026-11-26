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
};

/** What `sync_with_base` did. `conflicts` lists files still unmerged when it stopped. */
export type SyncOutcome = {
  pushed: boolean;
  conflicts: string[];
  message: string;
};
