import { useEffect, useRef } from "react";
import type { ApprovalRequestEvent } from "../types/orchestration";

export type LogLine = {
  id: number;
  ts: number;
  nodeId: string | null;
  kind: string;
  message: string;
};

export function RunLogPane({ lines }: { lines: LogLine[] }) {
  const endRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    endRef.current?.scrollIntoView({ block: "end" });
  }, [lines.length]);

  if (lines.length === 0) {
    return <div className="p-3 text-xs text-zinc-500">Run events appear here as agents act.</div>;
  }
  return (
    <ol className="space-y-1 p-2 font-mono text-[11px] leading-5">
      {lines.map((line) => (
        <li key={line.id} className="flex gap-2">
          <span className="shrink-0 text-zinc-600">{new Date(line.ts).toLocaleTimeString()}</span>
          <span className="shrink-0 text-indigo-300">{line.nodeId ?? "run"}</span>
          <span className="min-w-0 break-words text-zinc-300">{line.message}</span>
        </li>
      ))}
      <div ref={endRef} />
    </ol>
  );
}

export function ApprovalQueue({
  requests,
  nameOf,
  onDecide,
}: {
  requests: ApprovalRequestEvent[];
  nameOf: (nodeId: string) => string;
  onDecide: (request: ApprovalRequestEvent, approved: boolean) => void;
}) {
  if (requests.length === 0) return null;
  return (
    <div className="space-y-2 border-t border-amber-500/30 bg-amber-500/5 p-2">
      <div className="text-[11px] font-semibold uppercase tracking-wider text-amber-300">Approval required</div>
      {requests.map((request, index) => (
        <div key={`${request.edgeId}-${index}`} className="rounded-md border border-amber-500/30 bg-zinc-900 p-2.5">
          <div className="text-xs text-zinc-200">
            {nameOf(request.fromNodeId)} <span className="text-zinc-500">→</span> {nameOf(request.targetNodeId)}
          </div>
          <p className="mt-1 line-clamp-3 whitespace-pre-line text-[11px] text-zinc-400">{request.prompt}</p>
          <div className="mt-2 flex gap-2">
            <button
              type="button"
              onClick={() => onDecide(request, true)}
              className="rounded-md bg-emerald-600 px-2.5 py-1 text-xs font-medium text-white hover:bg-emerald-500"
            >
              Approve
            </button>
            <button
              type="button"
              onClick={() => onDecide(request, false)}
              className="rounded-md border border-red-500/50 px-2.5 py-1 text-xs font-medium text-red-300 hover:bg-red-500/10"
            >
              Reject
            </button>
          </div>
        </div>
      ))}
    </div>
  );
}
