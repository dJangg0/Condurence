import type { PullRequestStatus } from "../types/pulls";

/** What a manual button on one agent worktree does. Nothing here runs on its own. */
export type ChangeAction = "none" | "create" | "commit-and-create";

/**
 * The manual PR action for one worktree. The base branch itself has no PR to open, so it offers
 * nothing. Every other branch offers Create PR, or commit and create when it has uncommitted edits,
 * which needs a confirmation. A branch with an open or merged PR offers nothing, and so does a
 * branch whose PR status is still unknown (`pr` is `undefined`).
 */
export function changeAction(input: {
  branch: string | null;
  base: string;
  changedFiles: number;
  pr: PullRequestStatus | null | undefined;
}): ChangeAction {
  if (!input.branch || input.branch === input.base) return "none";
  if (input.pr === undefined) return "none";
  if (input.pr && (input.pr.state === "OPEN" || input.pr.state === "MERGED")) return "none";
  return input.changedFiles > 0 ? "commit-and-create" : "create";
}

/** Commit message for an agent's uncommitted edits: the run's goal on the first line. */
export function commitMessageFor(goal: string): string {
  const line = goal.trim().split("\n")[0] ?? "";
  const short = line.length > 72 ? `${line.slice(0, 71)}…` : line;
  return `Agent changes: ${short || "orchestration run"}`;
}

/**
 * Commit message for a workspace's pending edits, which have no run goal to describe them. It names the
 * file when there is only one, and counts the files otherwise.
 */
export function workspaceCommitMessage(paths: readonly string[]): string {
  if (paths.length === 0) return "Update workspace";
  if (paths.length === 1) return `Update ${paths[0]}`;
  return `Update ${paths.length} files`;
}

export type PrItemId = "create" | "commit-and-create" | "branch-and-create" | "open" | "copy-link" | "copy-branch";

export type PrItem = {
  id: PrItemId;
  label: string;
  /** An item that goes to a page rather than doing something here. */
  href?: string;
};

/** The split button: the main action, and the extra choices under its chevron. */
export type PrControl = {
  primary: PrItem & { disabled: boolean };
  menu: PrItem[];
};

const COPY_BRANCH: PrItem = { id: "copy-branch", label: "Copy branch name" };

/**
 * What the Create PR split button offers for one workspace, or `null` when there is nothing to
 * offer. The main action follows `changeAction`. A workspace on the base branch itself, such as the
 * repository's main checkout where the root and shared agents work, cannot open a PR from it, so
 * when it holds work it offers to move that work onto a new branch first. Under the chevron,
 * a workspace with uncommitted edits can still open its PR from the committed work alone, and one
 * that has a PR can go to it or copy its link. Every item is the user's to press.
 */
export function prControl(input: {
  branch: string | null;
  base: string;
  changedFiles: number;
  /** Files the branch has committed since the base. Only the base branch itself needs it. */
  committedFiles?: number;
  pr: PullRequestStatus | null | undefined;
}): PrControl | null {
  if (!input.branch) return null;
  if (input.branch === input.base) {
    if ((input.committedFiles ?? 0) + input.changedFiles === 0) return null;
    return {
      primary: { id: "branch-and-create", label: "Create branch & PR…", disabled: false },
      menu: [COPY_BRANCH],
    };
  }

  const { pr } = input;
  if (pr && (pr.state === "OPEN" || pr.state === "MERGED")) {
    return {
      primary: { id: "open", label: `PR #${pr.number} · ${pr.state.toLowerCase()}`, href: pr.url, disabled: false },
      menu: [{ id: "copy-link", label: "Copy PR link" }, COPY_BRANCH],
    };
  }

  const action = changeAction(input);
  if (action === "commit-and-create") {
    const files = `${input.changedFiles} uncommitted file${input.changedFiles === 1 ? "" : "s"}`;
    return {
      primary: { id: "commit-and-create", label: "Commit & create PR…", disabled: false },
      menu: [{ id: "create", label: `Create PR without the ${files}` }, COPY_BRANCH],
    };
  }
  // `none` here means the PR status is still unknown: show the button, but do not offer it yet.
  return { primary: { id: "create", label: "Create PR", disabled: action === "none" }, menu: [COPY_BRANCH] };
}

/** Every file the workspace changes, once: committed against the base plus uncommitted edits. */
export function changedPaths(committed: readonly { path: string }[] | undefined, uncommitted: readonly { path: string }[] | undefined): string[] {
  return [...new Set([...(committed ?? []), ...(uncommitted ?? [])].map((file) => file.path))];
}

/** The branch a run's work on the base branch moves to: named for the run, like its agents' `oc/` branches. */
export function defaultBranchName(runId: string): string {
  return `oc/${runId.slice(0, 8)}/changes`;
}

/** Whether git would take the name. The backend checks the same rules, and git has the last word. */
export function validBranchName(name: string): boolean {
  return (
    name.length > 0 &&
    !name.startsWith("-") &&
    !name.startsWith("/") &&
    !name.endsWith("/") &&
    !name.endsWith(".") &&
    !name.endsWith(".lock") &&
    !name.includes("..") &&
    !name.includes("//") &&
    !name.includes("@{") &&
    !/[\s\x00-\x1f~^:?*[\\]/.test(name)
  );
}

/** How much a run has changed, as GitHub's diffstat shows it: lines added and removed. */
export type RunChangeCount = {
  additions: number;
  deletions: number;
  /** Files changed, each counted once. */
  files: number;
  /** Workspaces holding any change. */
  workspaces: number;
};

type Lines = { additions: number; deletions: number };
type CountedFile = { path: string } & Lines;

/**
 * The size of a run's changes for the toolbar. Within one workspace, a file's committed and
 * uncommitted lines add up, as one edit on top of another. Across workspaces a file counts once, at
 * the workspace that changed it most: an isolated agent branches from its parent's commits, so its
 * diff repeats them and summing would count the same lines twice. Siblings that edit the same file
 * independently are therefore counted as the larger of the two, which is a lower bound.
 */
export function runChangeCount(
  workspaces: readonly { committed: readonly CountedFile[]; uncommitted: readonly CountedFile[] }[],
): RunChangeCount {
  const size = (lines: Lines) => lines.additions + lines.deletions;
  const perFile = new Map<string, Lines>();
  let holding = 0;
  for (const workspace of workspaces) {
    const own = new Map<string, Lines>();
    for (const file of [...workspace.committed, ...workspace.uncommitted]) {
      const sum = own.get(file.path) ?? { additions: 0, deletions: 0 };
      own.set(file.path, { additions: sum.additions + file.additions, deletions: sum.deletions + file.deletions });
    }
    if (own.size > 0) holding += 1;
    for (const [path, lines] of own) {
      const seen = perFile.get(path);
      if (!seen || size(lines) > size(seen)) perFile.set(path, lines);
    }
  }
  let additions = 0;
  let deletions = 0;
  for (const lines of perFile.values()) {
    additions += lines.additions;
    deletions += lines.deletions;
  }
  return { additions, deletions, files: perFile.size, workspaces: holding };
}
