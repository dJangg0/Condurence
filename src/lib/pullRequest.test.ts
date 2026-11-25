import { describe, expect, it } from "vitest";
import { pullButton } from "./pullRequest";
import type { PullRequestStatus } from "../types/pulls";

const open: PullRequestStatus = {
  number: 7,
  url: "https://github.com/o/r/pull/7",
  state: "OPEN",
  isDraft: false,
  mergeable: "MERGEABLE",
  mergeStateStatus: "CLEAN",
  reviewDecision: null,
  checks: "passing",
};

describe("pullButton", () => {
  it("offers Create PR when the branch has no PR, and waits while the first read is pending", () => {
    expect(pullButton(null)).toMatchObject({ label: "Create PR", action: "create" });
    expect(pullButton(undefined)).toMatchObject({ label: "Loading…", action: null });
  });

  it("turns into Merge only once the PR is mergeable and every check passes", () => {
    expect(pullButton(open)).toMatchObject({ label: "Merge", action: "merge", tone: "success" });
  });

  it("shows Checks running while checks are pending, without an action", () => {
    expect(pullButton({ ...open, checks: "pending" })).toMatchObject({ label: "Checks running", action: null });
  });

  it("shows Checks failing and blocks the merge when a check fails", () => {
    expect(pullButton({ ...open, checks: "failing" })).toMatchObject({ label: "Checks failing", action: null });
  });

  it("changes the button to Resolve when the PR conflicts, even while checks are failing", () => {
    const conflicting = { ...open, mergeable: "CONFLICTING", mergeStateStatus: "DIRTY", checks: "failing" as const };
    expect(pullButton(conflicting)).toMatchObject({ label: "Resolve", action: "resolve" });
    expect(pullButton({ ...open, mergeable: "CONFLICTING", mergeStateStatus: "UNKNOWN" })).toMatchObject({
      label: "Resolve",
      action: "resolve",
    });
  });

  it("waits for GitHub to finish computing mergeability before deciding", () => {
    expect(pullButton({ ...open, mergeable: "UNKNOWN", mergeStateStatus: "UNKNOWN" })).toMatchObject({
      label: "Checking…",
      action: null,
    });
  });

  it("treats drafts, branch-protection blocks and merged PRs as state, not actions", () => {
    expect(pullButton({ ...open, isDraft: true })).toMatchObject({ label: "Draft", action: null });
    expect(pullButton({ ...open, mergeStateStatus: "BLOCKED" })).toMatchObject({ label: "Blocked", action: null });
    expect(pullButton({ ...open, state: "MERGED" })).toMatchObject({ label: "Merged", action: null });
  });

  it("offers a new PR after the previous one was closed", () => {
    expect(pullButton({ ...open, state: "CLOSED" })).toMatchObject({ label: "Create PR", action: "create" });
  });
});
