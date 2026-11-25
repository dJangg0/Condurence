import type { ReactNode } from "react";
import {
  PROVIDERS,
  PROVIDER_LABELS,
  RELATION_HELP,
  RELATION_LABELS,
  RELATION_TYPES,
  type AgentProvider,
  type RelationType,
  type WorktreeMode,
} from "../types/orchestration";
import { MAX_RETRIES_LIMIT, type AgentFlowNode, type RoutingFlowEdge } from "../lib/graph";

const FIELD =
  "w-full rounded-md border border-zinc-700 bg-zinc-950 px-2.5 py-1.5 text-sm text-zinc-100 outline-none focus:border-indigo-500";

function Field({ label, hint, children }: { label: string; hint?: string; children: ReactNode }) {
  return (
    <label className="block space-y-1">
      <span className="text-xs font-medium text-zinc-400">{label}</span>
      {children}
      {hint ? <span className="block text-[11px] leading-snug text-zinc-500">{hint}</span> : null}
    </label>
  );
}

function listToText(values: string[]): string {
  return values.join(", ");
}

function textToList(value: string): string[] {
  return value
    .split(",")
    .map((item) => item.trim())
    .filter(Boolean);
}

export function NodeInspector({
  node,
  onChange,
  onDelete,
}: {
  node: AgentFlowNode;
  onChange: (patch: Partial<AgentFlowNode["data"]>) => void;
  onDelete: () => void;
}) {
  const data = node.data;
  return (
    <div className="space-y-4">
      <div className="flex items-start justify-between gap-2">
        <div>
          <div className="text-[11px] uppercase tracking-wider text-zinc-500">Agent node</div>
          <div className="font-mono text-xs text-zinc-400">{node.id}</div>
        </div>
        <button
          type="button"
          onClick={onDelete}
          className="rounded-md border border-red-500/40 px-2 py-1 text-xs text-red-300 hover:bg-red-500/10"
        >
          Delete
        </button>
      </div>

      <Field label="Name">
        <input className={FIELD} value={data.name} onChange={(e) => onChange({ name: e.target.value })} />
      </Field>
      <Field label="Role mandate">
        <input
          className={FIELD}
          value={data.role}
          placeholder="e.g. Senior Developer"
          onChange={(e) => onChange({ role: e.target.value })}
        />
      </Field>

      <div className="grid grid-cols-2 gap-3">
        <Field label="Provider">
          <select
            className={FIELD}
            value={data.provider}
            onChange={(e) => onChange({ provider: e.target.value as AgentProvider })}
          >
            {PROVIDERS.map((provider) => (
              <option key={provider} value={provider}>
                {PROVIDER_LABELS[provider]}
              </option>
            ))}
          </select>
        </Field>
        <Field label="Model" hint="Passed to the CLI as-is. Empty uses its default.">
          <input className={FIELD} value={data.model} placeholder="opus" onChange={(e) => onChange({ model: e.target.value })} />
        </Field>
      </div>

      <Field label="Worktree isolation" hint="Shared reuses the upstream worktree (live edits). Isolated gets a child branch.">
        <select
          className={FIELD}
          value={data.worktreeMode}
          onChange={(e) => onChange({ worktreeMode: e.target.value as WorktreeMode })}
        >
          <option value="shared">Shared with upstream</option>
          <option value="isolated">Isolated child worktree</option>
        </select>
      </Field>

      <Field label="Skills" hint="Comma-separated. Resolved from .orchestrator/skills/<name>.md.">
        <input
          className={FIELD}
          value={listToText(data.skills)}
          onChange={(e) => onChange({ skills: textToList(e.target.value) })}
        />
      </Field>
      <Field label="Context paths" hint="Comma-separated repo paths, e.g. AGENTS.md, src/">
        <input
          className={FIELD}
          value={listToText(data.contextPaths)}
          onChange={(e) => onChange({ contextPaths: textToList(e.target.value) })}
        />
      </Field>
      <Field label="System prompt">
        <textarea
          className={`${FIELD} min-h-24 font-mono text-xs`}
          value={data.systemPrompt}
          onChange={(e) => onChange({ systemPrompt: e.target.value })}
        />
      </Field>
    </div>
  );
}

export function EdgeInspector({
  edge,
  sourceName,
  targetName,
  onChange,
  onDelete,
}: {
  edge: RoutingFlowEdge;
  sourceName: string;
  targetName: string;
  onChange: (patch: Partial<RoutingFlowEdge["data"] & object>) => void;
  onDelete: () => void;
}) {
  const data = edge.data ?? { relationType: "delegates_to" as RelationType, maxRetries: 3, requiresApproval: false };
  return (
    <div className="space-y-4">
      <div className="flex items-start justify-between gap-2">
        <div>
          <div className="text-[11px] uppercase tracking-wider text-zinc-500">Routing edge</div>
          <div className="text-sm text-zinc-200">
            {sourceName} <span className="text-zinc-500">→</span> {targetName}
          </div>
        </div>
        <button
          type="button"
          onClick={onDelete}
          className="rounded-md border border-red-500/40 px-2 py-1 text-xs text-red-300 hover:bg-red-500/10"
        >
          Delete
        </button>
      </div>

      <Field label="Relation" hint={RELATION_HELP[data.relationType]}>
        <select
          className={FIELD}
          value={data.relationType}
          onChange={(e) => onChange({ relationType: e.target.value as RelationType })}
        >
          {RELATION_TYPES.map((relation) => (
            <option key={relation} value={relation}>
              {RELATION_LABELS[relation]}
            </option>
          ))}
        </select>
      </Field>

      <Field
        label="Max retries"
        hint="Maximum times this edge may fire in one run, including the first attempt. Guards loops."
      >
        <input
          className={FIELD}
          type="number"
          min={0}
          max={MAX_RETRIES_LIMIT}
          value={data.maxRetries}
          onChange={(e) => onChange({ maxRetries: Number.parseInt(e.target.value, 10) || 0 })}
        />
      </Field>

      <label className="flex items-start gap-2.5 rounded-md border border-zinc-800 bg-zinc-950/60 p-3">
        <input
          type="checkbox"
          className="mt-0.5 accent-amber-400"
          checked={data.requiresApproval}
          onChange={(e) => onChange({ requiresApproval: e.target.checked })}
        />
        <span className="space-y-0.5">
          <span className="block text-sm text-zinc-200">Requires human approval</span>
          <span className="block text-[11px] text-zinc-500">The run pauses on this handoff until you approve it.</span>
        </span>
      </label>
    </div>
  );
}
