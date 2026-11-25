import type { Edge, Node } from "@xyflow/react";
import {
  RELATION_LABELS,
  type AgentNodeData,
  type OrchestrationGraph,
  type RoutingEdgeData,
  type WorktreeMode,
} from "../types/orchestration";

export type AgentFlowNode = Node<AgentNodeData, "agentNode">;
export type RoutingFlowEdge = Edge<RoutingEdgeData>;

export const NODE_ID_PATTERN = /^[A-Za-z0-9_-]{1,32}$/;
export const MAX_RETRIES_LIMIT = 20;

export function defaultAgentData(name = "New agent"): AgentNodeData {
  return {
    name,
    role: "",
    provider: "claude",
    model: "",
    skills: [],
    contextPaths: [],
    systemPrompt: "",
    worktreeMode: "isolated",
    status: "idle",
  };
}

export function defaultEdgeData(): RoutingEdgeData {
  return { relationType: "delegates_to", maxRetries: 3, requiresApproval: false };
}

/** Visual treatment per relation: solid for delegation, dashed for verification, dotted for escalation. */
export function edgePresentation(data: RoutingEdgeData): Partial<RoutingFlowEdge> {
  const color = data.requiresApproval ? "#f59e0b" : "#a1a1aa";
  const dash = data.relationType === "verifies" ? "6 4" : data.relationType === "escalates_to" ? "2 4" : undefined;
  const suffix = data.requiresApproval ? " · approval" : "";
  return {
    label: `${RELATION_LABELS[data.relationType]} · ${data.maxRetries}×${suffix}`,
    style: { stroke: color, strokeWidth: 1.75, strokeDasharray: dash },
    labelStyle: { fill: "#d4d4d8", fontSize: 11 },
    labelBgStyle: { fill: "#18181b" },
    labelBgPadding: [6, 3],
    labelBgBorderRadius: 4,
    markerEnd: { type: "arrowclosed" as const, color },
  };
}

export function toFlow(graph: OrchestrationGraph): { nodes: AgentFlowNode[]; edges: RoutingFlowEdge[] } {
  return {
    nodes: graph.nodes.map((node) => ({
      id: node.id,
      type: "agentNode",
      position: node.position,
      data: node.data,
    })),
    edges: graph.edges.map((edge) => ({
      id: edge.id,
      source: edge.source,
      target: edge.target,
      data: edge.data,
      ...edgePresentation(edge.data),
    })),
  };
}

/** Builds the persisted shape. Runtime status is dropped so saves stay deterministic. */
export function fromFlow(
  nodes: AgentFlowNode[],
  edges: RoutingFlowEdge[],
  defaultWorktreeStrategy: WorktreeMode,
): OrchestrationGraph {
  return {
    version: 1,
    defaultWorktreeStrategy,
    nodes: nodes.map((node) => ({
      id: node.id,
      type: "agentNode",
      position: { x: Math.round(node.position.x), y: Math.round(node.position.y) },
      data: { ...node.data, status: "idle" },
    })),
    edges: edges.map((edge) => ({
      id: edge.id,
      source: edge.source,
      target: edge.target,
      data: {
        relationType: edge.data?.relationType ?? "delegates_to",
        maxRetries: edge.data?.maxRetries ?? 0,
        requiresApproval: edge.data?.requiresApproval ?? false,
      },
    })),
  };
}

/** Mirrors the backend validation so problems surface before a save round-trip. */
export function validateGraph(graph: OrchestrationGraph): string[] {
  const problems: string[] = [];
  const ids = new Set<string>();
  for (const node of graph.nodes) {
    if (!NODE_ID_PATTERN.test(node.id)) {
      problems.push(`Node id "${node.id}" must be 1-32 letters, digits, "-" or "_".`);
    }
    if (ids.has(node.id)) problems.push(`Duplicate node id "${node.id}".`);
    ids.add(node.id);
    if (!node.data.name.trim()) problems.push(`Node "${node.id}" needs a name.`);
  }
  const edgeIds = new Set<string>();
  for (const edge of graph.edges) {
    if (edgeIds.has(edge.id)) problems.push(`Duplicate edge id "${edge.id}".`);
    edgeIds.add(edge.id);
    if (!ids.has(edge.source) || !ids.has(edge.target)) {
      problems.push(`Edge "${edge.id}" references a missing node.`);
    }
    if (edge.source === edge.target) problems.push(`Edge "${edge.id}" connects a node to itself.`);
    if (!Number.isInteger(edge.data.maxRetries) || edge.data.maxRetries < 0 || edge.data.maxRetries > MAX_RETRIES_LIMIT) {
      problems.push(`Edge "${edge.id}" maxRetries must be an integer from 0 to ${MAX_RETRIES_LIMIT}.`);
    }
  }
  return problems;
}

/** Lower-cases a display name into a valid, unused node id. */
export function uniqueNodeId(name: string, existing: Iterable<string>): string {
  const taken = new Set(existing);
  const base =
    name
      .toLowerCase()
      .replace(/[^a-z0-9_-]+/g, "-")
      .replace(/^-+|-+$/g, "")
      .slice(0, 28) || "agent";
  if (!taken.has(base)) return base;
  let counter = 2;
  while (taken.has(`${base}-${counter}`)) counter += 1;
  return `${base}-${counter}`;
}

/** Nodes that nothing delegates to are natural roots; falls back to all nodes. */
export function rootCandidates(nodes: AgentFlowNode[], edges: RoutingFlowEdge[]): string[] {
  const delegatedTo = new Set(
    edges.filter((edge) => edge.data?.relationType === "delegates_to").map((edge) => edge.target),
  );
  const roots = nodes.filter((node) => !delegatedTo.has(node.id)).map((node) => node.id);
  return roots.length > 0 ? roots : nodes.map((node) => node.id);
}

export function hasDuplicateEdge(edges: RoutingFlowEdge[], source: string, target: string, relation: string): boolean {
  return edges.some(
    (edge) => edge.source === source && edge.target === target && edge.data?.relationType === relation,
  );
}
