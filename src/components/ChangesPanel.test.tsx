import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { ChangesPanel, ReviewDrawer } from "./ChangesPanel";
import { DEFAULT_REVIEW_OPTIONS } from "../lib/review";
import type { RunRecord } from "../types/orchestration";

const run: RunRecord = {
  id: "run-1",
  repoPath: "/repo",
  rootNodeId: "ceo",
  goal: "add dark mode",
  status: "finished",
  startedMs: 1,
  endedMs: 2,
};

describe("ChangesPanel", () => {
  it("opens as a full-width dialog with its diff modes and review control", () => {
    const markup = renderToStaticMarkup(
      <ChangesPanel run={run} initialPath="/repo/.worktrees/aatrox" transcript={[]} nameOf={(id) => id} onClose={() => undefined} />,
    );
    expect(markup).toContain('aria-label="Changes"');
    expect(markup).toContain("Changes · add dark mode");
    expect(markup).toContain("Pull request");
    expect(markup).toContain("Uncommitted");
    expect(markup).toContain(">Review</button>");
    expect(markup).toContain("Reading changes");
  });

  it("offers All files and Changes as tabs, opening on Changes", () => {
    const markup = renderToStaticMarkup(
      <ChangesPanel run={run} transcript={[]} nameOf={(id) => id} onClose={() => undefined} />,
    );
    expect(markup).toContain('role="tablist" aria-label="Review"');
    expect(markup).toMatch(/aria-selected="false"[^>]*>All files</);
    expect(markup).toMatch(/aria-selected="true"[^>]*>Changes</);
  });

  it("keeps Create PR out until it knows which workspace and branch it is for", () => {
    const markup = renderToStaticMarkup(
      <ChangesPanel run={run} transcript={[]} nameOf={(id) => id} onClose={() => undefined} />,
    );
    expect(markup).not.toContain("Create PR");
  });
});

describe("ReviewDrawer", () => {
  const drawer = (options = DEFAULT_REVIEW_OPTIONS) =>
    renderToStaticMarkup(
      <ReviewDrawer options={options} onChange={() => undefined} canStart={true} onStart={() => undefined} />,
    );

  it("offers the context the reviewer can be given, each as a checkbox", () => {
    const markup = drawer();
    for (const label of ["Committed changes", "Uncommitted edits", "Commit list", "Run goal", "Run handoffs"]) {
      expect(markup).toContain(label);
    }
    expect(markup).toContain('type="checkbox"');
    expect(markup).toContain("Remembered for this repository.");
  });

  it("lets the user choose the reviewer and the verification command", () => {
    const markup = drawer({ ...DEFAULT_REVIEW_OPTIONS, agent: "codex", testCommand: "npm test" });
    expect(markup).toContain('<option value="codex" selected="">Codex</option>');
    expect(markup).toContain('value="npm test"');
    expect(markup).toContain("Start review");
  });

  it("disables the start button when neither committed changes nor uncommitted edits are selected", () => {
    const markup = renderToStaticMarkup(
      <ReviewDrawer
        options={{ ...DEFAULT_REVIEW_OPTIONS, committed: false, uncommitted: false }}
        onChange={() => undefined}
        canStart={false}
        onStart={() => undefined}
      />,
    );
    expect(markup).toMatch(/disabled=""[^>]*>Start review/);
  });
});
