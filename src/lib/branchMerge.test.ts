import { describe, expect, it } from "vitest";
import { mergeStatus, pullForBranch } from "./branchMerge";
import type { BranchPull } from "../types/pulls";

const pull = (overrides: Partial<BranchPull>): BranchPull => ({
  branch: "oc/a/changes",
  number: 4,
  url: "https://github.com/o/r/pull/4",
  state: "OPEN",
  headSha: "abc",
  ...overrides,
});

describe("pullForBranch", () => {
  it("finds nothing for a branch without a PR", () => {
    expect(pullForBranch([pull({ branch: "other" })], "oc/a/changes")).toBeUndefined();
  });

  it("prefers an open PR over an older merged one on the same branch", () => {
    const merged = pull({ number: 2, state: "MERGED" });
    const open = pull({ number: 5, state: "OPEN" });
    expect(pullForBranch([merged, open], "oc/a/changes")?.number).toBe(5);
    expect(pullForBranch([merged], "oc/a/changes")?.number).toBe(2);
  });
});

describe("mergeStatus", () => {
  it("is merged when the PR merged and the branch is still at the merged commit", () => {
    expect(mergeStatus(pull({ state: "MERGED", headSha: "abc" }), "abc")).toMatchObject({ kind: "merged", label: "Merged" });
  });

  it("is not merged when commits came after the merge", () => {
    const status = mergeStatus(pull({ state: "MERGED", headSha: "abc" }), "def");
    expect(status).toMatchObject({ kind: "unmerged", label: "Not merged" });
    expect(status.hint).toContain("commits since");
  });

  it("trusts the PR's state when GitHub gave no head to compare", () => {
    expect(mergeStatus(pull({ state: "MERGED", headSha: null }), "def").kind).toBe("merged");
  });

  it("is an open PR while it waits to merge", () => {
    expect(mergeStatus(pull({ state: "OPEN" }), "abc")).toMatchObject({ kind: "open", label: "PR open" });
  });

  it("is not merged with no PR, or a PR closed without merging", () => {
    expect(mergeStatus(undefined, "abc")).toMatchObject({ kind: "unmerged", label: "Not merged" });
    expect(mergeStatus(pull({ state: "CLOSED" }), "abc")).toMatchObject({ kind: "unmerged", label: "Not merged" });
  });
});
