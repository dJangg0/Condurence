import { describe, expect, it } from "vitest";
import { describeHandoff, excerpt } from "./handoff";

describe("describeHandoff", () => {
  it("shows what the caller asked and what the target replied, on one line", () => {
    const line = describeHandoff(
      {
        from: "ceo",
        prompt: "## Delegated task (from `ceo`)\nAdd a single line containing exactly `hello, world` to README.md.\n\n## Target files\nREADME.md",
        status: "blocked",
        summary: "README.md is edited but not committed. The commit needs approval.",
      },
      "senior-dev",
    );

    expect(line).toBe(
      'ceo → senior-dev asked "Add a single line containing exactly `hello, world` to README.md." · senior-dev replied (blocked): "README.md is edited but not committed. The commit needs approval."',
    );
  });

  it("falls back to the status when the target sent no summary", () => {
    expect(describeHandoff({ from: "ceo", prompt: "Run the tests", status: "completed", summary: "" }, "senior-dev")).toBe(
      'ceo → senior-dev asked "Run the tests" · senior-dev completed',
    );
  });

  it("marks unknown endpoints instead of inventing them", () => {
    expect(describeHandoff({}, null)).toBe("? → ? · ? finished");
  });
});

describe("excerpt", () => {
  it("collapses whitespace and cuts long text with an ellipsis", () => {
    expect(excerpt("one\n\n  two\tthree")).toBe("one two three");
    const cut = excerpt("word ".repeat(100), 20);
    expect(cut.length).toBe(20);
    expect(cut.endsWith("…")).toBe(true);
  });
});
