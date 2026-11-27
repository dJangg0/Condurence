import { describe, expect, it } from "vitest";
import {
  changeAction,
  changedPaths,
  commitMessageFor,
  defaultBranchName,
  prControl,
  runChangeCount,
  validBranchName,
  workspaceCommitMessage,
} from "./changes";
import type { PullRequestStatus } from "../types/pulls";

const pr = (state: string): PullRequestStatus => ({
  number: 7,
  url: "https://github.com/acme/demo/pull/7",
  state,
  isDraft: false,
  mergeable: "MERGEABLE",
  mergeStateStatus: "CLEAN",
  reviewDecision: null,
  checks: "passing",
});

const base = { branch: "aatrox", base: "main", changedFiles: 0, pr: null };

describe("changeAction", () => {
  it("offers Create PR for any branch with no PR, including the main checkout's own feature branch", () => {
    expect(changeAction(base)).toBe("create");
    expect(changeAction({ ...base, branch: "feature/dark-mode" })).toBe("create");
  });

  it("offers commit and create when the worktree has uncommitted edits", () => {
    expect(changeAction({ ...base, changedFiles: 2 })).toBe("commit-and-create");
  });

  it("offers nothing for the base branch itself, a detached worktree, or an open or merged PR", () => {
    expect(changeAction({ ...base, branch: "main" })).toBe("none");
    expect(changeAction({ ...base, branch: null })).toBe("none");
    expect(changeAction({ ...base, pr: pr("OPEN") })).toBe("none");
    expect(changeAction({ ...base, changedFiles: 1, pr: pr("MERGED") })).toBe("none");
  });

  it("offers nothing while the PR status is unknown", () => {
    expect(changeAction({ ...base, pr: undefined })).toBe("none");
  });

  it("measures against the configured base, not a fixed branch name", () => {
    expect(changeAction({ ...base, branch: "beta", base: "beta" })).toBe("none");
    expect(changeAction({ ...base, branch: "main", base: "beta" })).toBe("create");
  });

  it("offers Create PR again once the old PR was closed", () => {
    expect(changeAction({ ...base, pr: pr("CLOSED") })).toBe("create");
  });
});

describe("commitMessageFor", () => {
  it("uses the goal's first line", () => {
    expect(commitMessageFor("add dark mode\nwith a toggle")).toBe("Agent changes: add dark mode");
  });

  it("shortens long goals and names the run when there is no goal", () => {
    expect(commitMessageFor("x".repeat(100))).toHaveLength("Agent changes: ".length + 72);
    expect(commitMessageFor("   ")).toBe("Agent changes: orchestration run");
  });
});

describe("prControl", () => {
  it("offers nothing for a detached worktree, or for the base branch while it has no work", () => {
    expect(prControl({ ...base, branch: null })).toBeNull();
    expect(prControl({ ...base, branch: "main" })).toBeNull();
    expect(prControl({ ...base, branch: "main", committedFiles: 0 })).toBeNull();
  });

  it("offers to move work on the base branch onto a new branch, whether it is committed or not", () => {
    const edits = prControl({ ...base, branch: "main", changedFiles: 2 });
    expect(edits?.primary).toMatchObject({ id: "branch-and-create", label: "Create branch & PR…", disabled: false });
    expect(prControl({ ...base, branch: "main", committedFiles: 3 })?.primary.id).toBe("branch-and-create");
    // The PR status of the base branch itself does not matter: it never has one.
    expect(prControl({ ...base, branch: "main", changedFiles: 1, pr: undefined })?.primary.disabled).toBe(false);
  });

  it("makes Create PR the main action for a clean branch with no PR", () => {
    const control = prControl(base);
    expect(control?.primary).toMatchObject({ id: "create", label: "Create PR", disabled: false });
    expect(control?.menu.map((item) => item.id)).toEqual(["copy-branch"]);
  });

  it("asks to commit first when there are uncommitted edits, but still allows the committed work alone", () => {
    const control = prControl({ ...base, changedFiles: 3 });
    expect(control?.primary).toMatchObject({ id: "commit-and-create", label: "Commit & create PR…" });
    expect(control?.menu[0]).toEqual({ id: "create", label: "Create PR without the 3 uncommitted files" });
    expect(prControl({ ...base, changedFiles: 1 })?.menu[0].label).toBe("Create PR without the 1 uncommitted file");
  });

  it("turns into a link to the PR once one is open or merged, with its link and branch to copy", () => {
    const open = prControl({ ...base, pr: pr("OPEN") });
    expect(open?.primary).toMatchObject({ id: "open", label: "PR #7 · open", href: "https://github.com/acme/demo/pull/7" });
    expect(open?.menu.map((item) => item.id)).toEqual(["copy-link", "copy-branch"]);
    expect(prControl({ ...base, changedFiles: 2, pr: pr("MERGED") })?.primary.label).toBe("PR #7 · merged");
  });

  it("offers Create PR again after the old PR was closed", () => {
    expect(prControl({ ...base, pr: pr("CLOSED") })?.primary).toMatchObject({ id: "create", disabled: false });
  });

  it("shows the button disabled while the PR status is unknown", () => {
    expect(prControl({ ...base, pr: undefined })?.primary).toMatchObject({ id: "create", disabled: true });
  });
});

describe("changedPaths", () => {
  it("counts a file once when it is both committed and edited again", () => {
    expect(changedPaths([{ path: "a.ts" }, { path: "b.ts" }], [{ path: "b.ts" }, { path: "c.ts" }])).toEqual(["a.ts", "b.ts", "c.ts"]);
    expect(changedPaths(undefined, undefined)).toEqual([]);
  });
});

describe("defaultBranchName and validBranchName", () => {
  it("names the branch after the run, beside the agents' own oc/ branches", () => {
    expect(defaultBranchName("0123456789abcdef")).toBe("oc/01234567/changes");
  });

  it("accepts ordinary branch names", () => {
    for (const name of ["oc/01234567/changes", "feature/dark-mode", "fix_1.2"]) expect(validBranchName(name)).toBe(true);
  });

  it("refuses names git would not take", () => {
    for (const name of ["", "-x", "/x", "x/", "x.", "x.lock", "a..b", "a//b", "a@{b", "has space", "a:b", "a~b", "a^b", "a?b", "a*b", "a[b", "a\\b", "a\tb"]) {
      expect(validBranchName(name), JSON.stringify(name)).toBe(false);
    }
  });
});

describe("runChangeCount", () => {
  const file = (path: string, additions: number, deletions: number) => ({ path, additions, deletions });

  it("is zero for a run that changed nothing", () => {
    const none = { additions: 0, deletions: 0, files: 0, workspaces: 0 };
    expect(runChangeCount([])).toEqual(none);
    expect(runChangeCount([{ committed: [], uncommitted: [] }])).toEqual(none);
  });

  it("totals the lines added and removed, committed and uncommitted together", () => {
    const count = runChangeCount([
      { committed: [file("a.ts", 10, 2), file("b.ts", 0, 5)], uncommitted: [file("c.ts", 3, 0)] },
    ]);
    expect(count).toEqual({ additions: 13, deletions: 7, files: 3, workspaces: 1 });
  });

  it("adds a file's committed lines to the edits made on top of them in the same workspace", () => {
    const count = runChangeCount([{ committed: [file("a.ts", 4, 1)], uncommitted: [file("a.ts", 2, 3)] }]);
    expect(count).toEqual({ additions: 6, deletions: 4, files: 1, workspaces: 1 });
  });

  it("counts a file once across workspaces, at the one that changed it most, because a child branch repeats its parent's commits", () => {
    const count = runChangeCount([
      { committed: [file("shared.ts", 5, 1)], uncommitted: [] },
      { committed: [file("shared.ts", 5, 1), file("own.ts", 7, 0)], uncommitted: [file("shared.ts", 2, 0)] },
      { committed: [], uncommitted: [] },
    ]);
    expect(count).toEqual({ additions: 14, deletions: 1, files: 2, workspaces: 2 });
  });

  it("counts a binary file as a changed file with no lines", () => {
    expect(runChangeCount([{ committed: [file("logo.png", 0, 0)], uncommitted: [] }])).toEqual({
      additions: 0,
      deletions: 0,
      files: 1,
      workspaces: 1,
    });
  });
});

describe("workspaceCommitMessage", () => {
  it("names the one file that changed", () => {
    expect(workspaceCommitMessage(["src/App.tsx"])).toBe("Update src/App.tsx");
  });

  it("counts the files when there are several", () => {
    expect(workspaceCommitMessage(["a.ts", "b.ts", "c.ts"])).toBe("Update 3 files");
  });

  it("has a message even when nothing is listed", () => {
    expect(workspaceCommitMessage([])).toBe("Update workspace");
  });
});
