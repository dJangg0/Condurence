import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { ArchiveWorkspaceDialog } from "./ArchiveWorkspaceDialog";
import type { WorktreeInfo } from "../types/orchestration";

const worktree = (overrides: Partial<WorktreeInfo> = {}): WorktreeInfo => ({
  path: "/repo/.worktrees/ahri",
  branch: "ahri",
  head: "0123456789abcdef",
  isMain: false,
  locked: false,
  ...overrides,
});

const render = (info = worktree()) =>
  renderToStaticMarkup(<ArchiveWorkspaceDialog repoPath="/repo" worktree={info} onArchived={() => undefined} onClose={() => undefined} />);

describe("ArchiveWorkspaceDialog", () => {
  it("opens as a dialog for the workspace and reads it before offering anything", () => {
    const markup = render();
    expect(markup).toContain('role="dialog"');
    expect(markup).toContain('aria-label="Archive ahri"');
    expect(markup).toContain("Checking the workspace…");
    // The confirming button stays disabled until the preview is read, so nothing is archived blind.
    expect(markup).toMatch(/disabled=""[^>]*autofocus[^>]*>Archive</i);
  });

  it("says that creating or merging a PR never archives a workspace", () => {
    expect(render()).toContain("Creating or merging its PR never does that for you.");
  });

  it("names a detached workspace by its commit", () => {
    expect(render(worktree({ branch: null }))).toContain("Archive detached 01234567");
  });
});
