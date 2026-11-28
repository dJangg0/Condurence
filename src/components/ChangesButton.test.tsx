import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { ChangesButton } from "./ChangesButton";
import type { RunChangeCount } from "../lib/changes";

const count = (overrides: Partial<RunChangeCount> = {}): RunChangeCount => ({ additions: 120, deletions: 34, files: 7, workspaces: 1, ...overrides });
const render = (value: RunChangeCount | null, hasRun = true) =>
  renderToStaticMarkup(<ChangesButton count={value} hasRun={hasRun} onClick={() => undefined} />);

describe("ChangesButton", () => {
  it("shows lines added in green and removed in red, as GitHub does", () => {
    const markup = render(count());
    expect(markup).toContain('<span class="text-emerald-400">+120</span>');
    expect(markup).toContain('<span class="text-red-400">−34</span>');
    expect(markup).toContain('aria-label="120 added, 34 removed"');
  });

  it("groups thousands", () => {
    const markup = render(count({ additions: 12345, deletions: 1000 }));
    expect(markup).toContain("+12,345");
    expect(markup).toContain("−1,000");
  });

  it("shows a removal-only change with its zero, so both sides are always there", () => {
    const markup = render(count({ additions: 0, deletions: 9 }));
    expect(markup).toContain("+0<");
    expect(markup).toContain("−9<");
  });

  it("explains the files and workspaces behind the lines in its title", () => {
    expect(render(count())).toContain('title="120 lines added, 34 removed in 7 files. Review them');
    expect(render(count({ files: 1 }))).toContain("in 1 file.");
    expect(render(count({ workspaces: 3 }))).toContain("in 7 files across 3 workspaces.");
  });

  it("shows the file count when the change has no text lines, such as a binary file", () => {
    const markup = render(count({ additions: 0, deletions: 0, files: 2 }));
    expect(markup).toContain("2 files");
    expect(markup).not.toContain("text-emerald-400");
  });

  it("stays plain, with no numbers, while nothing has changed or the count is not read yet", () => {
    for (const value of [count({ additions: 0, deletions: 0, files: 0, workspaces: 0 }), null]) {
      const markup = render(value);
      expect(markup).toContain("Changes");
      expect(markup).not.toContain("text-emerald-400");
      expect(markup).not.toContain("text-red-400");
    }
  });

  it("is disabled, with the reason, until there is a run", () => {
    const markup = render(null, false);
    expect(markup).toMatch(/disabled=""/);
    expect(markup).toContain("Run the topology first");
  });
});
