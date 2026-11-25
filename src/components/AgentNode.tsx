import { Handle, Position, type NodeProps } from "@xyflow/react";
import { PROVIDER_LABELS, type AgentProvider, type NodeStatus } from "../types/orchestration";
import type { AgentFlowNode } from "../lib/graph";

const ACCENT: Record<AgentProvider, string> = {
  claude: "bg-orange-400",
  codex: "bg-emerald-400",
  gemini: "bg-sky-400",
  opencode: "bg-violet-400",
};

const STATUS: Record<NodeStatus, { dot: string; label: string }> = {
  idle: { dot: "bg-zinc-500", label: "Idle" },
  running: { dot: "bg-blue-400 animate-pulse", label: "Running" },
  waiting_approval: { dot: "bg-amber-400 animate-pulse", label: "Waiting for approval" },
  error: { dot: "bg-red-500", label: "Error" },
};

const HANDLE_STYLE = { background: "#a1a1aa", width: 8, height: 8, border: "2px solid #18181b" };

/** Railway-style service card: provider accent rail, role, live status and routing mode. */
export function AgentNode({ data, selected }: NodeProps<AgentFlowNode>) {
  const status = STATUS[data.status];
  return (
    <div
      className={`w-60 rounded-lg border bg-zinc-900 shadow-lg shadow-black/40 transition-colors ${
        selected ? "border-indigo-400 ring-2 ring-indigo-500/30" : "border-zinc-700 hover:border-zinc-500"
      }`}
    >
      <Handle type="target" position={Position.Top} style={HANDLE_STYLE} />
      <div className="flex">
        <div className={`w-1.5 shrink-0 rounded-l-lg ${ACCENT[data.provider]}`} aria-hidden />
        <div className="min-w-0 flex-1 p-3">
          <div className="flex items-center justify-between gap-2">
            <span className="truncate text-sm font-semibold text-zinc-100">{data.name}</span>
            <span className={`h-2.5 w-2.5 shrink-0 rounded-full ${status.dot}`} title={status.label} aria-label={status.label} />
          </div>
          <div className="mt-0.5 truncate text-xs text-zinc-400">{data.role || "No role mandate"}</div>
          <div className="mt-2.5 flex flex-wrap items-center gap-1.5 text-[10px] font-medium uppercase tracking-wide">
            <span className="rounded bg-zinc-800 px-1.5 py-0.5 text-zinc-300" title={PROVIDER_LABELS[data.provider]}>
              {data.provider}
              {data.model ? ` · ${data.model}` : ""}
            </span>
            <span
              className={`rounded px-1.5 py-0.5 ${
                data.worktreeMode === "isolated" ? "bg-indigo-500/15 text-indigo-300" : "bg-zinc-800 text-zinc-400"
              }`}
            >
              {data.worktreeMode}
            </span>
          </div>
        </div>
      </div>
      <Handle type="source" position={Position.Bottom} style={HANDLE_STYLE} />
    </div>
  );
}
