import { describe, expect, it } from "vitest";
import { DEFAULT_REPO_SETTINGS, baseBranchOf } from "./repoSettings";

describe("repository settings", () => {
  it("start from origin/main, on origin, with upstream on push", () => {
    expect(DEFAULT_REPO_SETTINGS).toEqual({
      baseRef: "origin/main",
      remote: "origin",
      deleteBranchOnArchive: false,
      archiveOnMerge: false,
      setUpstreamOnPush: true,
    });
  });

  it("gives the base branch without its remote, keeping slashes in the branch name", () => {
    expect(baseBranchOf(DEFAULT_REPO_SETTINGS)).toBe("main");
    expect(baseBranchOf({ ...DEFAULT_REPO_SETTINGS, baseRef: "origin/release/1.0" })).toBe("release/1.0");
    expect(baseBranchOf({ ...DEFAULT_REPO_SETTINGS, baseRef: "beta" })).toBe("beta");
  });
});
