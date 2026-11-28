import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { FilesPane } from "./FilesPane";

describe("FilesPane", () => {
  it("opens with a search box, and says it is reading the files and waiting for a pick", () => {
    const markup = renderToStaticMarkup(
      <FilesPane worktreePath="/repo/.worktrees/aatrox" changed={new Map()} version={0} onShowDiff={() => undefined} />,
    );
    expect(markup).toContain('aria-label="Find a file"');
    expect(markup).toContain("Reading files…");
    expect(markup).toContain("Select a file to read it.");
  });
});
