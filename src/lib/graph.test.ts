import { describe, expect, it } from "vitest";
import {
  edgePresentation,
  fromFlow,
  hasDuplicateEdge,
  rootCandidates,
  toFlow,
  uniqueNodeId,
  validateGraph,
  type AgentFlowNode,
} from "./graph";
import type { OrchestrationGraph } from "../types/orchestration";

const sample: OrchestrationGraph = {
  version: 1,
  defaultWorktreeStrategy: "isolated",
  nodes: [
    {
      id: "ceo",
      type: "agentNode",
      position: { x: 260, y: 40 },
      data: {
        name: "CEO / Architect",
        role: "Primary Orchestrator",
        provider: "claude",
        model: "opus",
        skills: ["spec-planner"],
        contextPaths: ["AGENTS.md"],
        systemPrompt: "Decompose.",
        worktreeMode: "shared",
        status: "idle",
      },
    },
    {
      id: "senior-dev",
      type: "agentNode",
      position: { x: 80, y: 260 },
      data: {
        name: "Senior Developer",
        role: "Implementation",
        provider: "claude",
        model: "sonnet",
        skills: [],
        contextPaths: [],
        systemPrompt: "",
        worktreeMode: "isolated",
        status: "idle",
      },
    },
  ],
  edges: [
    {
      id: "e-ceo-senior-dev",
      source: "ceo",
      target: "senior-dev",
      data: { relationType: "delegates_to", maxRetries: 3, requiresApproval: false },
    },
  ],
};

describe("graph conversion", () => {
  it("round-trips the persisted shape through React Flow state", () => {
    const { nodes, edges } = toFlow(sample);
    const back = fromFlow(nodes, edges, "isolated");
    expect(back).toEqual(sample);
  });

  it("drops runtime status when persisting", () => {
    const { nodes, edges } = toFlow(sample);
    const running: AgentFlowNode[] = nodes.map((node) => ({ ...node, data: { ...node.data, status: "running" } }));
    const saved = fromFlow(running, edges, "shared");
    expect(saved.nodes.every((node) => node.data.status === "idle")).toBe(true);
    expect(saved.defaultWorktreeStrategy).toBe("shared");
  });

  it("rounds dragged positions to whole pixels", () => {
    const { nodes, edges } = toFlow(sample);
    const moved = nodes.map((node) => ({ ...node, position: { x: 10.7, y: 3.2 } }));
    expect(fromFlow(moved, edges, "isolated").nodes[0].position).toEqual({ x: 11, y: 3 });
  });

  it("styles verification edges dashed and approval edges amber", () => {
    const dashed = edgePresentation({ relationType: "verifies", maxRetries: 2, requiresApproval: false });
    expect(dashed.style?.strokeDasharray).toBe("6 4");
    const gated = edgePresentation({ relationType: "escalates_to", maxRetries: 1, requiresApproval: true });
    expect(gated.style?.stroke).toBe("#f59e0b");
    expect(String(gated.label)).toContain("approval");
  });
});

describe("validateGraph", () => {
  it("accepts the sample topology", () => {
    expect(validateGraph(sample)).toEqual([]);
  });

  it("flags bad ids, dangling edges, self-loops and retry limits", () => {
    const broken: OrchestrationGraph = {
      ...sample,
      nodes: [{ ...sample.nodes[0], id: "has space" }, sample.nodes[1]],
      edges: [
        { ...sample.edges[0], target: "ghost" },
        { id: "loop", source: "senior-dev", target: "senior-dev", data: { relationType: "verifies", maxRetries: 99, requiresApproval: false } },
      ],
    };
    const problems = validateGraph(broken);
    expect(problems.some((p) => p.includes("has space"))).toBe(true);
    expect(problems.some((p) => p.includes("missing node"))).toBe(true);
    expect(problems.some((p) => p.includes("itself"))).toBe(true);
    expect(problems.some((p) => p.includes("maxRetries"))).toBe(true);
  });
});

describe("authoring helpers", () => {
  it("generates unique, valid node ids", () => {
    expect(uniqueNodeId("Senior Developer", [])).toBe("senior-developer");
    expect(uniqueNodeId("Senior Developer", ["senior-developer"])).toBe("senior-developer-2");
    expect(uniqueNodeId("!!!", [])).toBe("agent");
    expect(uniqueNodeId("x".repeat(80), []).length).toBeLessThanOrEqual(28);
  });

  it("prefers nodes nothing delegates to as roots", () => {
    const { nodes, edges } = toFlow(sample);
    expect(rootCandidates(nodes, edges)).toEqual(["ceo"]);
  });

  it("detects duplicate relations between the same pair", () => {
    const { edges } = toFlow(sample);
    expect(hasDuplicateEdge(edges, "ceo", "senior-dev", "delegates_to")).toBe(true);
    expect(hasDuplicateEdge(edges, "ceo", "senior-dev", "verifies")).toBe(false);
  });
});
