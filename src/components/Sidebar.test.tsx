import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { Sidebar } from "./Sidebar";
import type { RepoRecord, WorktreeInfo } from "../types/orchestration";
import type { BranchPull } from "../types/pulls";

const REPO = "/repos/demo";
const repo: RepoRecord = { path: REPO, name: "demo", lastOpenedMs: 0, archived: false };

const worktrees: WorktreeInfo[] = [
  { path: `${REPO}`, branch: "main", head: "root000", isMain: true, locked: false },
  { path: `${REPO}/.worktrees/done`, branch: "oc/done/changes", head: "aaa111", isMain: false, locked: false },
  { path: `${REPO}/.worktrees/review`, branch: "oc/review/changes", head: "bbb222", isMain: false, locked: false },
  { path: `${REPO}/.worktrees/fresh`, branch: "oc/fresh/changes", head: "ccc333", isMain: false, locked: false },
];

const pulls: BranchPull[] = [
  { branch: "oc/done/changes", number: 3, url: "https://github.com/o/r/pull/3", state: "MERGED", headSha: "aaa111" },
  { branch: "oc/review/changes", number: 4, url: "https://github.com/o/r/pull/4", state: "OPEN", headSha: "bbb222" },
];

function markup(branchPulls: Record<string, BranchPull[]>): string {
  return renderToStaticMarkup(
    <Sidebar
      mode="workspaces"
      onModeChange={() => undefined}
      activePath={REPO}
      activeWorktreePath={null}
      recent={[repo]}
      archived={[]}
      worktrees={{ [REPO]: worktrees }}
      branchPulls={branchPulls}
      onOpenRepository={() => undefined}
      onSelectWorktree={() => undefined}
      onCreateWorkspace={() => undefined}
      onArchiveWorkspace={() => undefined}
      onOpenRepoSettings={() => undefined}
      onArchive={() => undefined}
      onRemove={() => undefined}
      settingsActive={false}
      onOpenSettings={() => undefined}
      connections={{ github: null, providers: null, error: null, refresh: () => undefined }}
      preferences={{ theme: "system", displayName: "" } as never}
      onPreferencesChange={() => undefined}
    />,
  );
}

/** The workspace row for one branch: its button markup, up to the next row. */
function rowFor(html: string, branch: string): string {
  const label = html.indexOf(`>${branch}<`);
  expect(label, `row for ${branch}`).toBeGreaterThan(-1);
  // The square comes before the label, so the row starts at its button.
  return html.slice(html.lastIndexOf("<button", label), html.indexOf("</button>", label));
}

describe("workspace merge indicators", () => {
  it("marks a merged workspace as merged, in its own color", () => {
    const html = markup({ [REPO]: pulls });
    const row = rowFor(html, "oc/done/changes");
    expect(row).toContain("Merged");
    expect(row).toContain("bg-emerald-400");
  });

  it("marks an open PR as open and not merged yet", () => {
    const row = rowFor(markup({ [REPO]: pulls }), "oc/review/changes");
    expect(row).toContain("PR open");
    expect(row).not.toContain("bg-emerald-400");
  });

  it("marks a workspace with no PR as not merged", () => {
    const row = rowFor(markup({ [REPO]: pulls }), "oc/fresh/changes");
    expect(row).toContain("Not merged");
    expect(row).not.toContain("Merged");
  });

  it("shows no merge state until the repository's PRs are read", () => {
    const html = markup({});
    expect(html).not.toContain("Merged");
    expect(html).not.toContain("Not merged");
    expect(html).not.toContain("PR open");
  });

  it("never gives the root checkout a merge state", () => {
    const row = rowFor(markup({ [REPO]: pulls }), "main");
    expect(row).not.toContain("Merged");
    expect(row).not.toContain("Not merged");
  });
});
