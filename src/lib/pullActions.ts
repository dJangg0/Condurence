import { ipc } from "./ipc";
import type { PullRequestStatus } from "../types/pulls";

/**
 * Opens a PR from a worktree's branch. Each step runs only after the user asked for the PR, and in
 * this order:
 *
 * 1. With `newBranch`, the worktree moves onto that new branch, bringing its commits and uncommitted
 *    edits. This is for work done on the base branch itself, which cannot be a PR's branch.
 * 2. With a commit message, the uncommitted edits are committed, so the PR carries them.
 * 3. The branch is pushed and the PR opened.
 */
export async function openPullRequest(input: {
  worktreePath: string;
  branch: string;
  base: string;
  remote: string;
  commitMessage: string | null;
  newBranch?: string | null;
}): Promise<PullRequestStatus> {
  let branch = input.branch;
  if (input.newBranch) {
    await ipc.startBranch(input.worktreePath, input.newBranch);
    branch = input.newBranch;
  }
  if (input.commitMessage) {
    await ipc.commitWorktreeChanges(input.worktreePath, input.commitMessage);
  }
  return ipc.createPullRequest(input.worktreePath, branch, input.base, input.remote);
}
