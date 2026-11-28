import { describe, expect, it } from "vitest";
import { goalSubmit } from "./goalInput";

const idle = { runActive: false, busy: false, hasAgents: true };

describe("goalSubmit", () => {
  it("starts a run from the typed goal when none is active", () => {
    expect(goalSubmit({ ...idle, text: "  add dark mode  " })).toEqual({ kind: "start", goal: "add dark mode" });
  });

  it("sends the text to the CEO while a run is live", () => {
    expect(goalSubmit({ text: "also add a test", runActive: true, busy: false, hasAgents: true })).toEqual({
      kind: "send",
      message: "also add a test",
    });
  });

  it("does nothing for an empty field, a busy canvas, or a topology with no agents", () => {
    expect(goalSubmit({ ...idle, text: "   " })).toEqual({ kind: "none" });
    expect(goalSubmit({ ...idle, text: "goal", busy: true })).toEqual({ kind: "none" });
    expect(goalSubmit({ ...idle, text: "goal", hasAgents: false })).toEqual({ kind: "none" });
  });

  it("never starts a second run from a live one", () => {
    expect(goalSubmit({ text: "goal", runActive: true, busy: false, hasAgents: true }).kind).toBe("send");
  });
});
