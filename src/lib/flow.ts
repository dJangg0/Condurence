import type { AgentNodeData, RelationType } from "../types/orchestration";

/**
 * Which way a message is travelling along an edge. `out` is a handoff in flight (caller to
 * target); `back` is the result returning (target to caller) for a moment after it finishes.
 */
export type EdgeFlow = "out" | "back";
export type EdgeFlowState = Record<string, EdgeFlow>;

type FlowEdge = { id: string; source: string; target: string; relationType: RelationType };

const TOOL_PREFIXES: Array<{ prefix: string; relation: RelationType }> = [
  { prefix: "delegate_to_", relation: "delegates_to" },
  { prefix: "verify_with_", relation: "verifies" },
  { prefix: "escalate_to_", relation: "escalates_to" },
];

/** Splits an edge tool name such as `delegate_to_greeter` into its relation and target node id. */
export function parseEdgeTool(tool: string): { relation: RelationType; target: string } | null {
  for (const { prefix, relation } of TOOL_PREFIXES) {
    if (tool.startsWith(prefix) && tool.length > prefix.length) {
      return { relation, target: tool.slice(prefix.length) };
    }
  }
  return null;
}

/** A node called an edge tool: the matching edge starts carrying a message toward its target. */
export function applyToolCall(
  state: EdgeFlowState,
  edges: readonly FlowEdge[],
  callerNodeId: string,
  tool: string,
): EdgeFlowState {
  const parsed = parseEdgeTool(tool);
  if (!parsed) return state;
  const edge = edges.find(
    (candidate) =>
      candidate.source === callerNodeId && candidate.target === parsed.target && candidate.relationType === parsed.relation,
  );
  return edge ? { ...state, [edge.id]: "out" } : state;
}

/** A node finished (or failed): messages that were travelling to it now travel back. */
export function applyStatus(
  state: EdgeFlowState,
  edges: readonly FlowEdge[],
  nodeId: string,
  status: AgentNodeData["status"],
): EdgeFlowState {
  if (status !== "idle" && status !== "error") return state;
  let next = state;
  for (const edge of edges) {
    if (edge.target === nodeId && state[edge.id] === "out") {
      next = { ...next, [edge.id]: "back" };
    }
  }
  return next;
}

/** Edges that are still returning a result, so the caller can clear them after a short delay. */
export function returningEdgeIds(state: EdgeFlowState): string[] {
  return Object.entries(state)
    .filter(([, flow]) => flow === "back")
    .map(([id]) => id);
}

export function clearEdges(state: EdgeFlowState, ids: readonly string[]): EdgeFlowState {
  if (!ids.some((id) => id in state)) return state;
  const next = { ...state };
  for (const id of ids) delete next[id];
  return next;
}

/** `0:07`, `1:05`. */
export function formatElapsed(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  return `${Math.floor(total / 60)}:${String(total % 60).padStart(2, "0")}`;
}
