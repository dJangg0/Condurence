import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Background,
  BackgroundVariant,
  Controls,
  ReactFlow,
  useEdgesState,
  useNodesState,
  type Connection,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { AgentNode } from "./AgentNode";
import { EdgeInspector, NodeInspector } from "./Inspector";
import { ApprovalQueue, RunLogPane, type LogLine } from "./RunPanel";
import { TerminalPane } from "./TerminalPane";
import { events, ipc } from "../lib/ipc";
import {
  defaultAgentData,
  defaultEdgeData,
  edgePresentation,
  fromFlow,
  hasDuplicateEdge,
  rootCandidates,
  toFlow,
  uniqueNodeId,
  validateGraph,
  type AgentFlowNode,
  type RoutingFlowEdge,
} from "../lib/graph";
import {
  RELATION_LABELS,
  type AgentNodeData,
  type ApprovalRequestEvent,
  type RoutingEdgeData,
  type RunEvent,
  type WorktreeMode,
} from "../types/orchestration";

const NODE_TYPES = { agentNode: AgentNode };

type Notice = { tone: "info" | "error"; text: string };
type Selection = { kind: "node" | "edge"; id: string } | null;
type ActiveRun = { runId: string; rootId: string };

function describeJournalEntry(kind: string, payload: Record<string, unknown>): string {
  switch (kind) {
    case "status":
      return `status → ${String(payload.status)}`;
    case "mcp_call":
      return `tool call ${String(payload.tool)}`;
    case "approval":
      return payload.approved ? "approval granted" : "approval rejected";
    default:
      return kind;
  }
}

/** Topology editor plus run controls. Nodes are railway-style cards; edges are routing rules. */
export function OrchestrationCanvas({ repoPath }: { repoPath: string }) {
  const [nodes, setNodes, onNodesChange] = useNodesState<AgentFlowNode>([]);
  const [edges, setEdges, onEdgesChange] = useEdgesState<RoutingFlowEdge>([]);
  const [worktreeDefault, setWorktreeDefault] = useState<WorktreeMode>("isolated");
  // Dirty is derived: the persisted-shape snapshot differs from what was last loaded or saved.
  const [savedSnapshot, setSavedSnapshot] = useState("");
  const snapshot = useMemo(
    () => JSON.stringify(fromFlow(nodes, edges, worktreeDefault)),
    [nodes, edges, worktreeDefault],
  );
  const dirty = snapshot !== savedSnapshot;
  const logSequence = useRef(0);
  const [selection, setSelection] = useState<Selection>(null);
  const [notice, setNotice] = useState<Notice | null>(null);
  const [rootId, setRootId] = useState("");
  const [goal, setGoal] = useState("");
  const [run, setRun] = useState<ActiveRun | null>(null);
  const [busy, setBusy] = useState(false);
  const [sessions, setSessions] = useState<Record<string, string>>({});
  const [focusNodeId, setFocusNodeId] = useState<string | null>(null);
  const [log, setLog] = useState<LogLine[]>([]);
  const [approvals, setApprovals] = useState<ApprovalRequestEvent[]>([]);
  const nodesRef = useRef(nodes);

  useEffect(() => {
    nodesRef.current = nodes;
  }, [nodes]);

  // Load the topology from disk whenever the repository changes.
  useEffect(() => {
    let cancelled = false;
    ipc
      .loadGraph(repoPath)
      .then((graph) => {
        if (cancelled) return;
        const flow = toFlow(graph);
        setNodes(flow.nodes);
        setEdges(flow.edges);
        setWorktreeDefault(graph.defaultWorktreeStrategy);
        setRootId(rootCandidates(flow.nodes, flow.edges)[0] ?? "");
        setSavedSnapshot(JSON.stringify(fromFlow(flow.nodes, flow.edges, graph.defaultWorktreeStrategy)));
        setSelection(null);
        setNotice({ tone: "info", text: "Topology loaded from .orchestrator/graph.json" });
      })
      .catch((error: unknown) => {
        if (!cancelled) setNotice({ tone: "error", text: String(error) });
      });
    return () => {
      cancelled = true;
    };
  }, [repoPath, setNodes, setEdges]);

  // While a run is active, mirror node statuses and collect run events.
  useEffect(() => {
    if (!run) return;
    let disposed = false;
    const unlisteners: Array<() => void> = [];
    const track = (unlisten: () => void) => {
      if (disposed) unlisten();
      else unlisteners.push(unlisten);
    };
    const { runId } = run;

    const applyStatus = (nodeId: string, status: AgentNodeData["status"], sessionId?: string | null) => {
      setNodes((current) =>
        current.map((node) => (node.id === nodeId ? { ...node, data: { ...node.data, status } } : node)),
      );
      if (sessionId) setSessions((current) => ({ ...current, [nodeId]: sessionId }));
    };

    for (const node of nodesRef.current) {
      void events
        .nodeStatus(node.id, (payload) => {
          if (payload.runId === runId) applyStatus(node.id, payload.status, payload.sessionId);
        })
        .then(track);
    }
    void events
      .runLog(runId, (entry) => {
        setLog((current) => [
          ...current.slice(-399),
          {
            id: (logSequence.current += 1),
            ts: entry.ts,
            nodeId: entry.nodeId,
            kind: entry.kind,
            message: describeJournalEntry(entry.kind, entry.payload),
          },
        ]);
      })
      .then(track);
    void events
      .approvalRequest(runId, (request) => setApprovals((current) => [...current, request]))
      .then(track);

    // Events may fire before we subscribed (the root starts during the start call), so
    // recover the latest status and terminal session per node from the journal.
    void ipc.listRunEvents(runId, 1000).then((journal: RunEvent[]) => {
      if (disposed) return;
      for (const event of journal) {
        if (event.kind === "status" && event.nodeId) {
          const status = event.payload.status as AgentNodeData["status"];
          const sessionId = typeof event.payload.sessionId === "string" ? event.payload.sessionId : null;
          applyStatus(event.nodeId, status, sessionId);
        }
      }
    });

    return () => {
      disposed = true;
      unlisteners.forEach((unlisten) => unlisten());
    };
  }, [run, setNodes]);

  const persist = useCallback(async (): Promise<boolean> => {
    const graph = fromFlow(nodes, edges, worktreeDefault);
    const problems = validateGraph(graph);
    if (problems.length > 0) {
      setNotice({ tone: "error", text: problems[0] });
      return false;
    }
    await ipc.saveGraph(repoPath, graph);
    setSavedSnapshot(JSON.stringify(graph));
    setNotice({ tone: "info", text: "Topology saved to .orchestrator/graph.json" });
    return true;
  }, [nodes, edges, worktreeDefault, repoPath]);

  const saveFromShortcut = useCallback(() => {
    persist().catch((error: unknown) => setNotice({ tone: "error", text: String(error) }));
  }, [persist]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "s") {
        event.preventDefault();
        saveFromShortcut();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [saveFromShortcut]);

  const onConnect = useCallback(
    (connection: Connection) => {
      const { source, target } = connection;
      if (!source || !target || source === target) return;
      const data = defaultEdgeData();
      setEdges((current) => {
        if (hasDuplicateEdge(current, source, target, data.relationType)) return current;
        const id = `e-${source}-${target}-${Math.random().toString(36).slice(2, 7)}`;
        return [...current, { id, source, target, data, ...edgePresentation(data) }];
      });
    },
    [setEdges],
  );

  const addAgent = () => {
    const id = uniqueNodeId("agent", nodes.map((node) => node.id));
    const offset = nodes.length;
    setNodes((current) => [
      ...current,
      {
        id,
        type: "agentNode",
        position: { x: 120 + (offset % 4) * 260, y: 60 + Math.floor(offset / 4) * 200 },
        data: defaultAgentData("New agent"),
      },
    ]);
    setSelection({ kind: "node", id });
  };

  const updateNode = (id: string, patch: Partial<AgentNodeData>) => {
    setNodes((current) => current.map((node) => (node.id === id ? { ...node, data: { ...node.data, ...patch } } : node)));
  };

  const deleteNode = (id: string) => {
    setNodes((current) => current.filter((node) => node.id !== id));
    setEdges((current) => current.filter((edge) => edge.source !== id && edge.target !== id));
    setSelection(null);
  };

  const updateEdge = (id: string, patch: Partial<RoutingEdgeData>) => {
    setEdges((current) =>
      current.map((edge) => {
        if (edge.id !== id) return edge;
        const data = { ...(edge.data ?? defaultEdgeData()), ...patch };
        return { ...edge, data, ...edgePresentation(data) };
      }),
    );
  };

  const deleteEdge = (id: string) => {
    setEdges((current) => current.filter((edge) => edge.id !== id));
    setSelection(null);
  };

  const reload = async () => {
    try {
      const graph = await ipc.loadGraph(repoPath);
      const flow = toFlow(graph);
      setNodes(flow.nodes);
      setEdges(flow.edges);
      setWorktreeDefault(graph.defaultWorktreeStrategy);
      setRootId((current) => (flow.nodes.some((n) => n.id === current) ? current : rootCandidates(flow.nodes, flow.edges)[0] ?? ""));
      setSavedSnapshot(JSON.stringify(fromFlow(flow.nodes, flow.edges, graph.defaultWorktreeStrategy)));
      setSelection(null);
      setNotice({ tone: "info", text: "Reloaded from disk" });
    } catch (error) {
      setNotice({ tone: "error", text: String(error) });
    }
  };

  const startRun = async () => {
    const trimmedGoal = goal.trim();
    if (!rootId) return setNotice({ tone: "error", text: "Add an agent before running the topology." });
    if (!trimmedGoal) return setNotice({ tone: "error", text: "Describe the goal for the root orchestrator." });
    setBusy(true);
    try {
      // The backend reads the topology from disk, so a dirty canvas must be saved first.
      if (dirty && !(await persist())) return;
      const runId = await ipc.startOrchestrationRun(repoPath, rootId, trimmedGoal);
      setSessions({});
      setLog([]);
      setApprovals([]);
      setFocusNodeId(rootId);
      setNodes((current) => current.map((node) => ({ ...node, data: { ...node.data, status: "idle" } })));
      setRun({ runId, rootId });
      setNotice({
        tone: "info",
        text: `Run started. "${rootId}" is interactive: converse with it in the terminal below.`,
      });
    } catch (error) {
      setNotice({ tone: "error", text: String(error) });
    } finally {
      setBusy(false);
    }
  };

  const stopRun = async () => {
    if (!run) return;
    setBusy(true);
    try {
      await ipc.stopOrchestrationRun(run.runId);
      setRun(null);
      setApprovals([]);
      setNotice({ tone: "info", text: "Run stopped. Agent sessions were terminated." });
    } catch (error) {
      setNotice({ tone: "error", text: String(error) });
    } finally {
      setBusy(false);
    }
  };

  const decide = async (request: ApprovalRequestEvent, approved: boolean) => {
    if (!run) return;
    try {
      await ipc.resolveApprovalGate(run.runId, request.edgeId, approved);
      setApprovals((current) => current.filter((item) => item.edgeId !== request.edgeId));
    } catch (error) {
      setNotice({ tone: "error", text: String(error) });
    }
  };

  const nameOf = (nodeId: string) => nodes.find((node) => node.id === nodeId)?.data.name ?? nodeId;
  const selectedNode = selection?.kind === "node" ? nodes.find((node) => node.id === selection.id) : undefined;
  const selectedEdge = selection?.kind === "edge" ? edges.find((edge) => edge.id === selection.id) : undefined;
  const problems = validateGraph(fromFlow(nodes, edges, worktreeDefault));
  const focusSession = focusNodeId ? sessions[focusNodeId] : undefined;
  const focusIsInteractiveRoot = run !== null && focusNodeId === run.rootId;

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex flex-wrap items-center gap-2 border-b border-zinc-800 bg-zinc-900/70 px-3 py-2">
        <button type="button" onClick={addAgent} className="rounded-md bg-zinc-800 px-2.5 py-1.5 text-xs font-medium text-zinc-100 hover:bg-zinc-700">
          + Agent
        </button>
        <button
          type="button"
          onClick={saveFromShortcut}
          disabled={!dirty}
          className="rounded-md bg-indigo-600 px-2.5 py-1.5 text-xs font-medium text-white hover:bg-indigo-500 disabled:cursor-not-allowed disabled:bg-zinc-800 disabled:text-zinc-500"
          title="Save topology (⌘S)"
        >
          {dirty ? "Save •" : "Saved"}
        </button>
        <button
          type="button"
          onClick={() => void reload()}
          disabled={dirty}
          className="rounded-md border border-zinc-700 px-2.5 py-1.5 text-xs text-zinc-300 hover:bg-zinc-800 disabled:opacity-40"
          title={dirty ? "Save or discard changes before reloading" : "Reload from disk"}
        >
          Reload
        </button>

        <div className="mx-1 h-5 w-px bg-zinc-700" />

        <label className="flex items-center gap-1.5 text-xs text-zinc-400">
          Root
          <select
            value={rootId}
            onChange={(e) => setRootId(e.target.value)}
            disabled={run !== null}
            className="rounded-md border border-zinc-700 bg-zinc-950 px-2 py-1 text-xs text-zinc-100"
          >
            {nodes.length === 0 ? <option value="">(no agents)</option> : null}
            {nodes.map((node) => (
              <option key={node.id} value={node.id}>
                {node.data.name}
              </option>
            ))}
          </select>
        </label>
        <input
          value={goal}
          onChange={(e) => setGoal(e.target.value)}
          placeholder="Goal for the orchestrator, e.g. add dark mode to settings"
          disabled={run !== null}
          className="min-w-48 flex-1 rounded-md border border-zinc-700 bg-zinc-950 px-2.5 py-1.5 text-xs text-zinc-100 outline-none placeholder:text-zinc-600 focus:border-indigo-500 disabled:opacity-50"
        />
        {run ? (
          <button type="button" onClick={() => void stopRun()} disabled={busy} className="rounded-md bg-red-600 px-3 py-1.5 text-xs font-medium text-white hover:bg-red-500 disabled:opacity-50">
            Stop run
          </button>
        ) : (
          <button
            type="button"
            onClick={() => void startRun()}
            disabled={busy || nodes.length === 0}
            className="rounded-md bg-emerald-600 px-3 py-1.5 text-xs font-medium text-white hover:bg-emerald-500 disabled:opacity-50"
          >
            Run topology
          </button>
        )}
      </div>

      {notice ? (
        <div
          className={`border-b px-3 py-1.5 text-xs ${
            notice.tone === "error" ? "border-red-500/30 bg-red-500/10 text-red-200" : "border-zinc-800 bg-zinc-900 text-zinc-400"
          }`}
        >
          {notice.text}
        </div>
      ) : null}

      <div className="flex min-h-0 flex-1">
        <div className="relative min-w-0 flex-1">
          <ReactFlow
            nodes={nodes}
            edges={edges}
            nodeTypes={NODE_TYPES}
            onNodesChange={onNodesChange}
            onEdgesChange={onEdgesChange}
            onConnect={onConnect}
            onNodeClick={(_, node) => {
              setSelection({ kind: "node", id: node.id });
              if (sessions[node.id]) setFocusNodeId(node.id);
            }}
            onEdgeClick={(_, edge) => setSelection({ kind: "edge", id: edge.id })}
            onPaneClick={() => setSelection(null)}
            fitView
            fitViewOptions={{ padding: 0.25 }}
            proOptions={{ hideAttribution: true }}
            deleteKeyCode={["Backspace", "Delete"]}
            defaultEdgeOptions={{ type: "smoothstep" }}
          >
            <Background variant={BackgroundVariant.Dots} gap={18} size={1.2} color="#3f3f46" />
            <Controls showInteractive={false} />
          </ReactFlow>
          {nodes.length === 0 ? (
            <div className="pointer-events-none absolute inset-0 flex items-center justify-center text-sm text-zinc-500">
              Add an agent, then drag from one node's handle to another to wire a routing rule.
            </div>
          ) : null}
        </div>

        <aside className="flex w-80 shrink-0 flex-col overflow-y-auto border-l border-zinc-800 bg-zinc-900/60 p-4">
          {selectedNode ? (
            <NodeInspector
              node={selectedNode}
              onChange={(patch) => updateNode(selectedNode.id, patch)}
              onDelete={() => deleteNode(selectedNode.id)}
            />
          ) : selectedEdge ? (
            <EdgeInspector
              edge={selectedEdge}
              sourceName={nameOf(selectedEdge.source)}
              targetName={nameOf(selectedEdge.target)}
              targetWorktreeMode={nodes.find((node) => node.id === selectedEdge.target)?.data.worktreeMode ?? "isolated"}
              onChange={(patch) => updateEdge(selectedEdge.id, patch)}
              onDelete={() => deleteEdge(selectedEdge.id)}
            />
          ) : (
            <div className="space-y-4">
              <div>
                <div className="text-[11px] uppercase tracking-wider text-zinc-500">Topology</div>
                <div className="mt-1 text-sm text-zinc-300">
                  {nodes.length} agents · {edges.length} routes
                </div>
              </div>
              <label className="block space-y-1">
                <span className="text-xs font-medium text-zinc-400">Default worktree strategy</span>
                <select
                  value={worktreeDefault}
                  onChange={(e) => {
                    setWorktreeDefault(e.target.value as WorktreeMode);
                  }}
                  className="w-full rounded-md border border-zinc-700 bg-zinc-950 px-2.5 py-1.5 text-sm text-zinc-100"
                >
                  <option value="isolated">Isolated</option>
                  <option value="shared">Shared</option>
                </select>
              </label>
              <div className="space-y-1.5 text-xs text-zinc-500">
                <div className="font-medium text-zinc-400">Relations</div>
                {Object.entries(RELATION_LABELS).map(([key, label]) => (
                  <div key={key}>{label}</div>
                ))}
              </div>
              {problems.length > 0 ? (
                <div className="rounded-md border border-red-500/30 bg-red-500/5 p-2.5 text-xs text-red-300">
                  {problems.map((problem) => (
                    <div key={problem}>{problem}</div>
                  ))}
                </div>
              ) : (
                <div className="text-xs text-emerald-400/80">Topology is valid.</div>
              )}
              <div className="text-xs text-zinc-500">Select a node or edge to edit it.</div>
            </div>
          )}
        </aside>
      </div>

      <div className="flex h-72 shrink-0 border-t border-zinc-800">
        <div className="flex min-w-0 flex-1 flex-col">
          <div className="flex items-center gap-2 border-b border-zinc-800 px-3 py-1.5 text-xs text-zinc-400">
            <span className="font-medium text-zinc-300">Terminal</span>
            {focusNodeId ? <span className="font-mono text-indigo-300">{nameOf(focusNodeId)}</span> : null}
            {focusIsInteractiveRoot ? <span className="rounded bg-emerald-500/15 px-1.5 py-0.5 text-[10px] text-emerald-300">interactive</span> : null}
            {focusSession && focusNodeId && !focusIsInteractiveRoot ? (
              <span className="rounded bg-zinc-800 px-1.5 py-0.5 text-[10px] text-zinc-400">read-only stream</span>
            ) : null}
          </div>
          <div className="min-h-0 flex-1">
            {focusSession ? (
              <TerminalPane key={focusSession} sessionId={focusSession} interactive={focusIsInteractiveRoot} />
            ) : (
              <div className="p-4 text-xs text-zinc-500">
                {run ? "Waiting for agents to start…" : "Run the topology to open live agent terminals. Click a node to inspect its stream."}
              </div>
            )}
          </div>
        </div>
        <div className="flex w-96 shrink-0 flex-col border-l border-zinc-800">
          <div className="border-b border-zinc-800 px-3 py-1.5 text-xs font-medium text-zinc-300">Run log</div>
          <div className="min-h-0 flex-1 overflow-y-auto">
            <RunLogPane lines={log} />
          </div>
          <ApprovalQueue requests={approvals} nameOf={nameOf} onDecide={(request, approved) => void decide(request, approved)} />
        </div>
      </div>
    </div>
  );
}
