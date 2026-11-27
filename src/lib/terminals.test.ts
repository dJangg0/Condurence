import { describe, expect, it } from "vitest";
import { sessionsIn, shownSession } from "./terminals";

const claude = { id: "a", cwd: "/w/ahri", label: "Claude" };
const shell = { id: "b", cwd: "/w/ahri", label: "Shell" };
const beirut = { id: "c", cwd: "/w/beirut", label: "Claude" };
const all = [claude, beirut, shell];

describe("sessionsIn", () => {
  it("keeps only one workspace's sessions, in the order they started", () => {
    expect(sessionsIn(all, "/w/ahri")).toEqual([claude, shell]);
  });

  it("has none without a workspace", () => {
    expect(sessionsIn(all, null)).toEqual([]);
  });
});

describe("shownSession", () => {
  it("shows the picked session while it is open in this workspace", () => {
    expect(shownSession(all, "a", "/w/ahri")).toBe(claude);
  });

  it("falls back to the most recently started session when nothing was picked", () => {
    expect(shownSession(all, undefined, "/w/ahri")).toBe(shell);
  });

  it("ignores a pick that belongs to another workspace", () => {
    expect(shownSession(all, "c", "/w/ahri")).toBe(shell);
  });

  it("is null for a workspace with no sessions, or without a workspace", () => {
    expect(shownSession(all, undefined, "/w/nowhere")).toBeNull();
    expect(shownSession(all, undefined, null)).toBeNull();
  });

  it("keeps a session running in another workspace when this one starts its own", () => {
    const after = [...all, { id: "d", cwd: "/w/beirut", label: "Shell" }];
    expect(sessionsIn(after, "/w/beirut").map((s) => s.id)).toEqual(["c", "d"]);
    expect(sessionsIn(after, "/w/ahri")).toEqual([claude, shell]);
  });
});
