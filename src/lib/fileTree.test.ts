import { describe, expect, it } from "vitest";
import { ancestorDirs, baseName, dirsHolding, matchFiles, treeRows } from "./fileTree";

const FILES = [
  "README.md",
  "package.json",
  "src/App.tsx",
  "src/lib/ipc.ts",
  "src/lib/changes.ts",
  "src/components/Alpha.tsx",
  "docs/SRS.md",
];

const shape = (rows: ReturnType<typeof treeRows>) => rows.map((row) => `${"  ".repeat(row.depth)}${row.kind === "dir" ? (row.open ? "v " : "> ") : ""}${row.name}`);

describe("treeRows", () => {
  it("lists directories before files, each by name, with every directory closed", () => {
    expect(shape(treeRows(FILES, new Set()))).toEqual(["> docs", "> src", "package.json", "README.md"]);
  });

  it("opens a directory to show its own directories first, then its files", () => {
    const rows = treeRows(FILES, new Set(["src", "src/lib"]));
    expect(shape(rows)).toEqual([
      "> docs",
      "v src",
      "  > components",
      "  v lib",
      "    changes.ts",
      "    ipc.ts",
      "  App.tsx",
      "package.json",
      "README.md",
    ]);
    expect(rows.find((row) => row.name === "ipc.ts")?.path).toBe("src/lib/ipc.ts");
  });

  it("hides the contents of a closed directory even when a deeper one is open", () => {
    expect(shape(treeRows(FILES, new Set(["src/lib"])))).toEqual(["> docs", "> src", "package.json", "README.md"]);
  });

  it("orders numbered names naturally and ignores case", () => {
    const rows = treeRows(["b10.txt", "B2.txt", "a.txt"], new Set());
    expect(rows.map((row) => row.name)).toEqual(["a.txt", "B2.txt", "b10.txt"]);
  });

  it("is empty for no files", () => {
    expect(treeRows([], new Set())).toEqual([]);
  });
});

describe("ancestorDirs and dirsHolding", () => {
  it("lists the directories above a file, outermost first", () => {
    expect(ancestorDirs("src/lib/a.ts")).toEqual(["src", "src/lib"]);
    expect(ancestorDirs("README.md")).toEqual([]);
  });

  it("collects every directory that holds one of the paths", () => {
    expect([...dirsHolding(["src/lib/a.ts", "src/App.tsx", "README.md"])].sort()).toEqual(["src", "src/lib"]);
  });
});

describe("baseName", () => {
  it("takes the last segment", () => {
    expect(baseName("src/lib/a.ts")).toBe("a.ts");
    expect(baseName("a.ts")).toBe("a.ts");
  });
});

describe("matchFiles", () => {
  it("needs every word in the path, ignoring case", () => {
    expect(matchFiles(FILES, "SRC lib")).toEqual(["src/lib/ipc.ts", "src/lib/changes.ts"].sort((a, b) => a.length - b.length || a.localeCompare(b)));
    expect(matchFiles(FILES, "zzz")).toEqual([]);
  });

  it("puts files named for the first word before files that only sit in a matching folder", () => {
    expect(matchFiles(["lib/changes/readme.md", "src/changes.ts"], "changes")[0]).toBe("src/changes.ts");
  });

  it("returns nothing for a blank query and caps the result", () => {
    expect(matchFiles(FILES, "   ")).toEqual([]);
    expect(matchFiles(FILES, "s", 2)).toHaveLength(2);
  });
});
