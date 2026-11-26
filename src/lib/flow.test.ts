import { describe, expect, it } from "vitest";
import {
  applyStatus,
  applyToolCall,
  clearEdges,
  formatElapsed,
  parseEdgeTool,
  returningEdgeIds,
  type EdgeFlowState,
} from "./flow";

const edges = [
  { id: "e1", source: "ceo", target: "dev", relationType: "delegates_to" as const },
  { id: "e2", source: "dev", target: "qa", relationType: "verifies" as const },
  { id: "e3", source: "dev", target: "ceo", relationType: "escalates_to" as const },
];

describe("parseEdgeTool", () => {
  it("splits each relation's tool name, keeping ids that contain dashes and underscores", () => {
    expect(parseEdgeTool("delegate_to_senior-dev")).toEqual({ relation: "delegates_to", target: "senior-dev" });
    expect(parseEdgeTool("verify_with_qa_bot")).toEqual({ relation: "verifies", target: "qa_bot" });
    expect(parseEdgeTool("escalate_to_ceo")).toEqual({ relation: "escalates_to", target: "ceo" });
  });
  it("ignores other tools", () => {
    expect(parseEdgeTool("report_task_result")).toBeNull();
    expect(parseEdgeTool("delegate_to_")).toBeNull();
  });
});

describe("edge flow", () => {
  it("a delegation lights the caller's edge, then the result flows back, then it clears", () => {
    let state: EdgeFlowState = {};
    state = applyToolCall(state, edges, "ceo", "delegate_to_dev");
    expect(state).toEqual({ e1: "out" });

    state = applyStatus(state, edges, "dev", "running");
    expect(state).toEqual({ e1: "out" });

    state = applyStatus(state, edges, "dev", "idle");
    expect(state).toEqual({ e1: "back" });
    expect(returningEdgeIds(state)).toEqual(["e1"]);

    expect(clearEdges(state, ["e1"])).toEqual({});
  });

  it("only the matching caller, target and relation light up", () => {
    expect(applyToolCall({}, edges, "qa", "delegate_to_dev")).toEqual({});
    expect(applyToolCall({}, edges, "ceo", "verify_with_dev")).toEqual({});
    expect(applyToolCall({}, edges, "ceo", "delegate_to_nobody")).toEqual({});
    expect(applyToolCall({}, edges, "dev", "verify_with_qa")).toEqual({ e2: "out" });
  });

  it("a failed target also returns the message, and unrelated edges are untouched", () => {
    let state: EdgeFlowState = { e1: "out", e2: "out" };
    state = applyStatus(state, edges, "dev", "error");
    expect(state).toEqual({ e1: "back", e2: "out" });
  });

  it("a node that was never called leaves the state alone", () => {
    const state: EdgeFlowState = { e1: "out" };
    expect(applyStatus(state, edges, "qa", "idle")).toBe(state);
  });
});

describe("formatElapsed", () => {
  it("formats minutes and seconds", () => {
    expect(formatElapsed(7_000)).toBe("0:07");
    expect(formatElapsed(65_000)).toBe("1:05");
    expect(formatElapsed(-5)).toBe("0:00");
  });
});

describe("a real run", () => {
  // The journal of the live Claude-to-Claude run (ceo delegates hello to greeter), in order.
  const live = [
    { kind: "status", nodeId: "ceo", status: "running" },
    { kind: "mcp_call", nodeId: "ceo", tool: "delegate_to_greeter" },
    { kind: "status", nodeId: "greeter", status: "running" },
    { kind: "mcp_call", nodeId: "greeter", tool: "report_task_result" },
    { kind: "status", nodeId: "greeter", status: "idle" },
  ] as const;
  const topology = [{ id: "e-ceo-greeter", source: "ceo", target: "greeter", relationType: "delegates_to" as const }];

  it("animates out while the greeter works, back when it reports, and ignores the report tool", () => {
    let state: EdgeFlowState = {};
    const seen: EdgeFlowState[] = [];
    for (const event of live) {
      state =
        event.kind === "mcp_call"
          ? applyToolCall(state, topology, event.nodeId, event.tool)
          : applyStatus(state, topology, event.nodeId, event.status);
      seen.push(state);
    }
    expect(seen.map((s) => s["e-ceo-greeter"])).toEqual([undefined, "out", "out", "out", "back"]);
  });
});
