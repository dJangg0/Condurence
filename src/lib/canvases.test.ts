import { describe, expect, it } from "vitest";
import { canvasPaths, withoutCanvas } from "./canvases";

describe("canvasPaths", () => {
  it("mounts nothing until Agent Orchestration has been opened", () => {
    expect(canvasPaths([], { repoPath: "/a", orchestrating: false })).toEqual([]);
    expect(canvasPaths([], { repoPath: null, orchestrating: true })).toEqual([]);
  });

  it("adds the repository the first time Agent Orchestration opens for it", () => {
    expect(canvasPaths([], { repoPath: "/a", orchestrating: true })).toEqual(["/a"]);
  });

  it("keeps the canvas while the user is in another view, so its run is not lost", () => {
    expect(canvasPaths(["/a"], { repoPath: "/a", orchestrating: false })).toEqual(["/a"]);
  });

  it("keeps the canvas of a repository the user moved away from", () => {
    expect(canvasPaths(["/a"], { repoPath: "/b", orchestrating: false })).toEqual(["/a"]);
    expect(canvasPaths(["/a"], { repoPath: "/b", orchestrating: true })).toEqual(["/a", "/b"]);
  });

  it("never lists a repository twice, and keeps the order, so no canvas is remounted", () => {
    expect(canvasPaths(["/a", "/b"], { repoPath: "/a", orchestrating: true })).toEqual(["/a", "/b"]);
  });
});

describe("withoutCanvas", () => {
  it("drops only the repository that was archived or removed", () => {
    expect(withoutCanvas(["/a", "/b"], "/a")).toEqual(["/b"]);
    expect(withoutCanvas(["/a"], "/zzz")).toEqual(["/a"]);
  });
});
