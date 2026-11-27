import { describe, expect, it } from "vitest";
import { transcriptItems } from "./history";
import type { RunEvent } from "../types/orchestration";

const event = (id: number, kind: string, payload: Record<string, unknown>, nodeId: string | null = null): RunEvent => ({
  id,
  runId: "run-1",
  nodeId,
  kind,
  payload,
  createdMs: 1_000 + id,
});

describe("transcriptItems", () => {
  it("turns a handoff into what was asked and what came back", () => {
    const [item] = transcriptItems([
      event(
        1,
        "handoff",
        {
          relation: "delegates_to",
          from: "ceo",
          to: "senior-dev",
          prompt: "Add a greeting line",
          status: "completed",
          exitCode: 0,
          summary: "Added the line",
          diffStat: " README.md | 1 +",
        },
        "senior-dev",
      ),
    ]);
    expect(item).toEqual({
      kind: "handoff",
      key: "event-1",
      at: 1_001,
      from: "ceo",
      to: "senior-dev",
      relation: "delegates_to",
      prompt: "Add a greeting line",
      status: "completed",
      summary: "Added the line",
      diffStat: " README.md | 1 +",
      exitCode: 0,
    });
  });

  it("keeps approval decisions and leaves status and tool-call bookkeeping out", () => {
    const items = transcriptItems([
      event(1, "status", { status: "running" }, "ceo"),
      event(2, "mcp_call", { tool: "delegate_to_senior-dev", instance: "ceo.0" }, "ceo"),
      event(3, "approval", { edgeId: "e-ceo-dev", approved: true }),
      event(4, "approval", { edgeId: "e-ceo-dev", approved: false }),
    ]);
    expect(items.map((item) => item.kind)).toEqual(["approval", "approval"]);
    expect(items[0]).toMatchObject({ approved: true, edgeId: "e-ceo-dev" });
    expect(items[1]).toMatchObject({ approved: false });
  });

  it("tolerates a handoff written without its optional fields", () => {
    const [item] = transcriptItems([event(7, "handoff", { from: "ceo", to: "dev" })]);
    expect(item).toMatchObject({ prompt: "", summary: "", diffStat: "", status: "", exitCode: null });
  });

  it("keeps the journal's order", () => {
    const items = transcriptItems([
      event(5, "handoff", { from: "ceo", to: "a" }),
      event(6, "handoff", { from: "a", to: "b" }),
    ]);
    expect(items.map((item) => (item.kind === "handoff" ? item.to : ""))).toEqual(["a", "b"]);
  });
});
