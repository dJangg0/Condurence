import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { Position, type EdgeProps } from "@xyflow/react";
import { EdgeFlowContext, FlowEdge } from "./FlowEdge";
import type { EdgeFlowState } from "../lib/flow";

const props = {
  id: "e1",
  source: "a",
  target: "b",
  sourceX: 0,
  sourceY: 0,
  targetX: 100,
  targetY: 200,
  sourcePosition: Position.Bottom,
  targetPosition: Position.Top,
  style: { stroke: "#a1a1aa" },
  markerEnd: "url(#arrow)",
} as unknown as EdgeProps;

function render(state: EdgeFlowState): string {
  return renderToStaticMarkup(
    <EdgeFlowContext.Provider value={state}>
      <svg>
        <FlowEdge {...props} />
      </svg>
    </EdgeFlowContext.Provider>,
  );
}

describe("FlowEdge", () => {
  it("is a plain line with no moving parts while idle", () => {
    const html = render({});
    expect(html).not.toContain("animateMotion");
    expect(html).not.toContain("oc-flow");
    expect(html).toContain("#a1a1aa");
  });

  it("sends three dots from the caller to the target while a handoff is in flight", () => {
    const html = render({ e1: "out" });
    expect(html.match(/<animateMotion/g)).toHaveLength(3);
    expect(html).toContain('keyPoints="0;1"');
    expect(html).toContain("oc-flow-out");
    expect(html).toContain("#818cf8");
  });

  it("runs the dots backwards, in green, while the result returns", () => {
    const html = render({ e1: "back" });
    expect(html).toContain('keyPoints="1;0"');
    expect(html).toContain("oc-flow-back");
    expect(html).toContain("#34d399");
  });

  it("only animates the edge that is carrying a message", () => {
    const html = render({ other: "out" });
    expect(html).not.toContain("animateMotion");
  });
});
