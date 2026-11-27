import { describe, expect, it } from "vitest";
import { archiveResult, archiveSummary, isInside, workspaceFolder } from "./archive";
import type { ArchivePreview } from "../types/orchestration";
import type { PullRequestStatus } from "../types/pulls";

const pr = (state: string): PullRequestStatus => ({
  number: 12,
  url: "https://github.com/acme/demo/pull/12",
  state,
  isDraft: false,
  mergeable: "UNKNOWN",
  mergeStateStatus: "UNKNOWN",
  reviewDecision: null,
  checks: "none",
});

const preview = (overrides: Partial<ArchivePreview> = {}): ArchivePreview => ({
  uncommitted: [],
  unpushed: 0,
  pr: null,
  delivered: false,
  ...overrides,
});

const keep = { branch: "ahri", deleteBranch: false };
const remove = { branch: "ahri", deleteBranch: true };

describe("workspaceFolder", () => {
  it("is the last part of the path", () => {
    expect(workspaceFolder("/repo/.worktrees/ahri")).toBe("ahri");
    expect(workspaceFolder("C:\\repo\\.worktrees\\ahri\\")).toBe("ahri");
  });
});

describe("isInside", () => {
  it("is true for the folder itself and for anything under it", () => {
    expect(isInside("/repo/.worktrees/ahri", "/repo/.worktrees/ahri")).toBe(true);
    expect(isInside("/repo/.worktrees/ahri/src/app", "/repo/.worktrees/ahri")).toBe(true);
    expect(isInside("/repo/.worktrees/ahri/src", "/repo/.worktrees/ahri/")).toBe(true);
    expect(isInside("C:\\repo\\.worktrees\\ahri\\src", "C:\\repo\\.worktrees\\ahri")).toBe(true);
  });

  it("is false for a sibling that only shares the start of the name", () => {
    expect(isInside("/repo/.worktrees/ahri-2", "/repo/.worktrees/ahri")).toBe(false);
    expect(isInside("/repo", "/repo/.worktrees/ahri")).toBe(false);
  });
});

describe("archiveSummary", () => {
  it("archives plainly when nothing would be lost", () => {
    const summary = archiveSummary(preview(), keep);
    expect(summary.discards).toEqual([]);
    expect(summary.confirmLabel).toBe("Archive");
    expect(summary.unpushed).toBeNull();
    expect(summary.pr).toBe("No pull request was found for this branch.");
    expect(summary.branch).toBe("The branch ahri is kept.");
  });

  it("says out loud that uncommitted files are discarded", () => {
    const summary = archiveSummary(preview({ uncommitted: ["a.ts", "b.ts"] }), keep);
    expect(summary.discards).toEqual(["a.ts", "b.ts"]);
    expect(summary.confirmLabel).toBe("Discard 2 files and archive");
    expect(archiveSummary(preview({ uncommitted: ["a.ts"] }), keep).confirmLabel).toBe("Discard 1 file and archive");
  });

  it("explains the PR, and that a merged one holds everything on the branch", () => {
    expect(archiveSummary(preview({ pr: pr("OPEN") }), keep).pr).toBe("PR #12 is open.");
    expect(archiveSummary(preview({ pr: pr("MERGED"), delivered: true }), keep).pr).toBe(
      "PR #12 is merged. Everything on the branch is in it.",
    );
    expect(archiveSummary(preview({ pr: pr("MERGED") }), keep).pr).toBe("PR #12 is merged.");
  });

  it("tells what becomes of the branch under each setting", () => {
    expect(archiveSummary(preview({ pr: pr("MERGED"), delivered: true }), remove).branch).toBe(
      "The branch ahri is deleted, because its PR is merged.",
    );
    expect(archiveSummary(preview(), remove).branch).toBe(
      "The branch ahri is deleted only if everything on it is merged. Otherwise it is kept.",
    );
    expect(archiveSummary(preview(), { branch: null, deleteBranch: true }).branch).toBe("This workspace has no branch.");
  });

  it("warns about commits GitHub does not have, and says they stay", () => {
    expect(archiveSummary(preview({ unpushed: 1 }), keep).unpushed).toBe(
      "1 commit on ahri is not on GitHub. They stay on the branch, which is not deleted.",
    );
    expect(archiveSummary(preview({ unpushed: 3 }), keep).unpushed).toContain("3 commits on ahri are not on GitHub");
  });

  it("does not warn about unpushed commits once the PR holds everything", () => {
    expect(archiveSummary(preview({ unpushed: 3, pr: pr("MERGED"), delivered: true }), keep).unpushed).toBeNull();
  });
});

describe("archiveResult", () => {
  it("reports what happened to the branch", () => {
    expect(archiveResult({ branchDeleted: true, branchNote: null }, "ahri")).toBe("Archived. The branch ahri was deleted.");
    expect(archiveResult({ branchDeleted: false, branchNote: null }, "ahri")).toBe("Archived. The branch ahri was kept.");
    expect(archiveResult({ branchDeleted: false, branchNote: "it has commits that are not merged into another branch" }, "ahri")).toBe(
      "Archived. The branch ahri was kept: it has commits that are not merged into another branch.",
    );
    expect(archiveResult({ branchDeleted: false, branchNote: null }, null)).toBe("Archived.");
  });
});
