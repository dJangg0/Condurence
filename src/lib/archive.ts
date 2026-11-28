import type { ArchiveOutcome, ArchivePreview } from "../types/orchestration";

/** The folder name of a workspace: the last part of its path. */
export function workspaceFolder(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
}

/** Whether `path` is `folder` itself or something inside it. */
export function isInside(path: string, folder: string): boolean {
  if (path === folder) return true;
  const base = folder.replace(/[\\/]+$/, "");
  return path.startsWith(`${base}/`) || path.startsWith(`${base}\\`);
}

const plural = (count: number, word: string) => `${count} ${word}${count === 1 ? "" : "s"}`;

export type ArchiveSummary = {
  /** What GitHub says about the branch's PR. */
  pr: string;
  /** Files that archiving will discard. Empty when there is nothing to lose. */
  discards: string[];
  /** Commits that are not on GitHub, and what happens to them. `null` when that does not apply. */
  unpushed: string | null;
  /** What becomes of the branch. */
  branch: string;
  /** The label of the confirming button. Losing changes is said out loud. */
  confirmLabel: string;
};

/**
 * What the confirmation tells the user before a workspace is archived: what is discarded, what
 * stays, and what happens to the branch. `deleteBranch` is the repository's "Delete branch on
 * archive" setting. Archiving is always the user's own action, and a PR being created or merged
 * never archives a workspace.
 */
export function archiveSummary(
  preview: ArchivePreview,
  input: { branch: string | null; deleteBranch: boolean },
): ArchiveSummary {
  const { branch, deleteBranch } = input;
  const pr = preview.pr
    ? `PR #${preview.pr.number} is ${preview.pr.state.toLowerCase()}.${preview.delivered ? " Everything on the branch is in it." : ""}`
    : "No pull request was found for this branch.";

  let branchLine: string;
  if (!branch) branchLine = "This workspace has no branch.";
  else if (!deleteBranch) branchLine = `The branch ${branch} is kept.`;
  else if (preview.delivered) branchLine = `The branch ${branch} is deleted, because its PR is merged.`;
  else branchLine = `The branch ${branch} is deleted only if everything on it is merged. Otherwise it is kept.`;

  const unpushed =
    branch && !preview.delivered && preview.unpushed > 0
      ? `${plural(preview.unpushed, "commit")} on ${branch} ${preview.unpushed === 1 ? "is" : "are"} not on GitHub. They stay on the branch, which is not deleted.`
      : null;

  const discards = preview.uncommitted;
  return {
    pr,
    discards,
    unpushed,
    branch: branchLine,
    confirmLabel: discards.length > 0 ? `Discard ${plural(discards.length, "file")} and archive` : "Archive",
  };
}

/** The line shown once the workspace is archived. */
export function archiveResult(outcome: ArchiveOutcome, branch: string | null): string {
  if (!branch) return "Archived.";
  if (outcome.branchDeleted) return `Archived. The branch ${branch} was deleted.`;
  if (outcome.branchNote) return `Archived. The branch ${branch} was kept: ${outcome.branchNote}.`;
  return `Archived. The branch ${branch} was kept.`;
}
