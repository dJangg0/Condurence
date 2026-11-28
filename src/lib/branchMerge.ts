import type { BranchPull } from "../types/pulls";

/** Whether a workspace's work has merged, as the sidebar shows it. */
export type MergeStatus = {
  kind: "merged" | "open" | "unmerged";
  label: string;
  hint: string;
};

/**
 * The PR that describes a branch. A branch can have several over time, so an open one comes first,
 * then a merged one, then whichever came first in the list.
 */
export function pullForBranch(pulls: readonly BranchPull[], branch: string): BranchPull | undefined {
  const mine = pulls.filter((pull) => pull.branch === branch);
  return mine.find((pull) => pull.state === "OPEN") ?? mine.find((pull) => pull.state === "MERGED") ?? mine[0];
}

/**
 * Whether a workspace is merged. A merged PR counts only while the branch is still where the PR left
 * it: commits made after the merge have not merged. This is the rule archiving uses for "delivered".
 * With no PR, or one that was closed without merging, the work is not merged.
 */
export function mergeStatus(pull: BranchPull | undefined, head: string): MergeStatus {
  if (!pull) return { kind: "unmerged", label: "Not merged", hint: "No pull request yet" };
  if (pull.state === "OPEN") {
    return { kind: "open", label: "PR open", hint: `PR #${pull.number} is open, not merged yet` };
  }
  if (pull.state === "MERGED") {
    if (!pull.headSha || pull.headSha === head) {
      return { kind: "merged", label: "Merged", hint: `PR #${pull.number} is merged` };
    }
    return {
      kind: "unmerged",
      label: "Not merged",
      hint: `PR #${pull.number} is merged, but this branch has commits since`,
    };
  }
  return { kind: "unmerged", label: "Not merged", hint: `PR #${pull.number} was closed without merging` };
}
