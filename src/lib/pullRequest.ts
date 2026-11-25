import type { PullRequestStatus } from "../types/pulls";

/** The branch every workspace PR targets. */
export const BASE_BRANCH = "main";

export type PullAction = "create" | "resolve" | "merge";

export type PullButton = {
  label: string;
  /** What a click does. `null` means the button only shows state. */
  action: PullAction | null;
  tone: "primary" | "success" | "warning" | "danger" | "muted";
  hint: string;
};

/**
 * The workspace's primary PR button. Conflicts come first because they block everything else.
 * Checks decide the rest, and "Merge" appears only once the PR is mergeable with every check green.
 * `undefined` means the first status read has not finished yet.
 */
export function pullButton(pr: PullRequestStatus | null | undefined): PullButton {
  if (pr === undefined) return { label: "Loading…", action: null, tone: "muted", hint: "Reading PR status from GitHub" };
  if (pr === null) {
    return { label: "Create PR", action: "create", tone: "primary", hint: `Push this branch and open a PR into ${BASE_BRANCH}` };
  }
  if (pr.state === "MERGED") return { label: "Merged", action: null, tone: "muted", hint: `PR #${pr.number} is merged` };
  if (pr.state !== "OPEN") {
    return { label: "Create PR", action: "create", tone: "primary", hint: `PR #${pr.number} was closed. Open a new one` };
  }
  if (pr.mergeable === "CONFLICTING" || pr.mergeStateStatus === "DIRTY") {
    return { label: "Resolve", action: "resolve", tone: "warning", hint: `Merge ${BASE_BRANCH} into this branch and fix the conflicts` };
  }
  if (pr.mergeable === "UNKNOWN") return { label: "Checking…", action: null, tone: "muted", hint: "GitHub is computing mergeability" };
  if (pr.isDraft) return { label: "Draft", action: null, tone: "muted", hint: "Mark the PR ready for review on GitHub" };
  if (pr.checks === "failing") return { label: "Checks failing", action: null, tone: "danger", hint: `Fix the failing checks on PR #${pr.number}` };
  if (pr.checks === "pending") return { label: "Checks running", action: null, tone: "muted", hint: "Merge unlocks when every check passes" };
  if (pr.mergeStateStatus === "BLOCKED") {
    return { label: "Blocked", action: null, tone: "muted", hint: "Branch protection needs approval or another requirement on GitHub" };
  }
  return { label: "Merge", action: "merge", tone: "success", hint: `Squash-merge PR #${pr.number}` };
}
