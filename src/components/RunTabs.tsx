import { formatElapsed } from "../lib/flow";
import type { NodeStatus } from "../types/orchestration";

export type AgentTab = {
  nodeId: string;
  name: string;
  status: NodeStatus;
  isRoot: boolean;
  /** Epoch ms when the agent last started running, if known. */
  startedAt?: number;
  /** Headline of its last result, shown once it has finished. */
  summary?: string;
};

const DOT: Record<NodeStatus, string> = {
  idle: "bg-emerald-400",
  running: "bg-blue-400 animate-pulse",
  waiting_approval: "bg-amber-400 animate-pulse",
  error: "bg-red-500",
};

const STATUS_WORD: Record<NodeStatus, string> = {
  idle: "finished",
  running: "working",
  waiting_approval: "waiting for your approval",
  error: "failed",
};

/** What the selected agent is doing, in one line. */
function caption(tab: AgentTab, now: number): { text: string; tone: "info" | "error" } {
  if (tab.status === "running") {
    const elapsed = tab.startedAt ? ` · ${formatElapsed(now - tab.startedAt)}` : "";
    return { text: `${tab.name} is ${STATUS_WORD.running}${elapsed}`, tone: "info" };
  }
  if (tab.status === "waiting_approval") {
    return { text: `${tab.name} is ${STATUS_WORD.waiting_approval}. Decide in the run panel.`, tone: "info" };
  }
  const detail = tab.summary ? `: ${tab.summary}` : "";
  if (tab.status === "error") return { text: `${tab.name} ${STATUS_WORD.error}${detail}`, tone: "error" };
  return { text: tab.isRoot ? `${tab.name} is ready for your next message` : `${tab.name} ${STATUS_WORD.idle}${detail}`, tone: "info" };
}

/**
 * One tab per agent that has started in this run, so the progress of every AI is a click away.
 * Each tab shows live status and elapsed time. With Follow on, the terminal switches to whichever
 * agent starts working and returns to the orchestrator when it finishes.
 */
export function RunTabs({
  tabs,
  focusNodeId,
  onFocus,
  follow,
  onFollowChange,
  now,
}: {
  tabs: AgentTab[];
  focusNodeId: string | null;
  onFocus: (nodeId: string) => void;
  follow: boolean;
  onFollowChange: (follow: boolean) => void;
  now: number;
}) {
  const focused = tabs.find((tab) => tab.nodeId === focusNodeId);
  const line = focused ? caption(focused, now) : null;

  return (
    <div className="border-b border-zinc-800">
      <div className="flex items-center gap-1 px-2 pt-1.5">
        <span className="mr-1 text-xs font-medium text-zinc-300">Terminal</span>
        <div role="tablist" aria-label="Agents" className="flex min-w-0 flex-1 flex-wrap gap-1">
          {tabs.map((tab) => {
            const selected = tab.nodeId === focusNodeId;
            return (
              <button
                key={tab.nodeId}
                type="button"
                role="tab"
                aria-selected={selected}
                onClick={() => onFocus(tab.nodeId)}
                title={tab.summary ? `${tab.name}: ${tab.summary}` : tab.name}
                className={`flex max-w-[11rem] items-center gap-1.5 rounded-t-md border border-b-0 px-2 py-1 text-xs ${
                  selected
                    ? "border-zinc-700 bg-zinc-950 text-zinc-100"
                    : "border-transparent text-zinc-400 hover:bg-zinc-800/60 hover:text-zinc-200"
                }`}
              >
                <span className={`h-2 w-2 shrink-0 rounded-full ${DOT[tab.status]}`} aria-hidden />
                <span className="truncate">{tab.name}</span>
                {tab.status === "running" && tab.startedAt ? (
                  <span className="shrink-0 font-mono text-[10px] text-zinc-500">{formatElapsed(now - tab.startedAt)}</span>
                ) : null}
              </button>
            );
          })}
        </div>
        <label className="flex shrink-0 items-center gap-1 text-[11px] text-zinc-400" title="Switch to whichever agent is working">
          <input
            type="checkbox"
            checked={follow}
            onChange={(event) => onFollowChange(event.target.checked)}
            className="accent-indigo-500"
          />
          Follow
        </label>
      </div>
      {line ? (
        <div
          className={`truncate border-t border-zinc-800 bg-zinc-900/60 px-3 py-1 text-[11px] ${
            line.tone === "error" ? "text-red-300" : "text-zinc-400"
          }`}
          title={line.text}
        >
          {line.text}
        </div>
      ) : null}
      {focused ? (
        <div className="border-t border-zinc-800 px-3 py-0.5 text-[10px] text-zinc-500">
          {focused.isRoot
            ? "Interactive: click in the terminal below and type to talk to the orchestrator."
            : "Read-only: this agent runs on its own. Switch to the orchestrator tab to talk."}
        </div>
      ) : null}
    </div>
  );
}
