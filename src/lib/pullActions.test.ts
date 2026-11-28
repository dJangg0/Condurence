import { beforeEach, describe, expect, it, vi } from "vitest";

const calls: string[] = [];
vi.mock("./ipc", () => ({
  ipc: {
    startBranch: vi.fn(async (path: string, branch: string) => void calls.push(`start ${path} ${branch}`)),
    commitWorktreeChanges: vi.fn(async (path: string, message: string) => {
      calls.push(`commit ${path} ${message}`);
      return "abc";
    }),
    createPullRequest: vi.fn(async (path: string, branch: string, base: string, remote: string) => {
      calls.push(`create ${path} ${branch} ${base} ${remote}`);
      return { number: 7 };
    }),
  },
}));

import { openPullRequest } from "./pullActions";

const input = { worktreePath: "/w", branch: "feature", base: "main", remote: "origin", commitMessage: null };

describe("openPullRequest", () => {
  beforeEach(() => {
    calls.length = 0;
  });

  it("only pushes and opens the PR when asked for nothing else", async () => {
    await openPullRequest(input);
    expect(calls).toEqual(["create /w feature main origin"]);
  });

  it("commits the pending edits first when given a message", async () => {
    await openPullRequest({ ...input, commitMessage: "Agent changes: x" });
    expect(calls).toEqual(["commit /w Agent changes: x", "create /w feature main origin"]);
  });

  it("moves work on the base branch onto a new branch, then commits, then opens the PR from the new branch", async () => {
    await openPullRequest({ ...input, branch: "main", commitMessage: "msg", newBranch: "oc/abc12345/changes" });
    expect(calls).toEqual(["start /w oc/abc12345/changes", "commit /w msg", "create /w oc/abc12345/changes main origin"]);
  });

  it("stops at the first step that fails, so nothing later runs", async () => {
    const { ipc } = await import("./ipc");
    vi.mocked(ipc.startBranch).mockRejectedValueOnce("a branch named oc/x already exists");
    await expect(openPullRequest({ ...input, branch: "main", newBranch: "oc/x", commitMessage: "m" })).rejects.toBe("a branch named oc/x already exists");
    expect(calls).toEqual([]);
  });
});
