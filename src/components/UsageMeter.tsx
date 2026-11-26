import { PROVIDER_LABELS, type ProviderUsage } from "../types/orchestration";
import { formatTokens, meterPercent, meteredTokens } from "../lib/usage";
import { PROVIDER_ACCENT } from "./AgentNode";

/**
 * One meter per provider the run used. Each bar shows that provider's tokens relative to the
 * busiest provider, so the strip answers "which CLI is carrying this run".
 */
export function UsageMeter({ rows, live }: { rows: ProviderUsage[]; live: boolean }) {
  const peak = Math.max(0, ...rows.map(meteredTokens));
  return (
    <div
      role="group"
      aria-label="Provider usage"
      className="flex items-center gap-5 overflow-x-auto border-b border-zinc-800 bg-zinc-900/60 px-3 py-2"
    >
      <div className="shrink-0">
        <div className="text-[11px] font-medium text-zinc-300">Usage</div>
        <div className="text-[10px] text-zinc-500">{live ? "This run" : "Last run"}</div>
      </div>
      {rows.map((row) => (
        <ProviderMeter key={row.provider} row={row} peak={peak} />
      ))}
    </div>
  );
}

function ProviderMeter({ row, peak }: { row: ProviderUsage; peak: number }) {
  const label = PROVIDER_LABELS[row.provider];

  if (!row.measured) {
    return (
      <div className="min-w-44 flex-1" title={`${label} does not expose token usage to Condurence yet.`}>
        <div className="text-xs text-zinc-300">{label}</div>
        <div className="mt-1 text-[11px] text-zinc-500">Not measured yet</div>
      </div>
    );
  }

  const total = meteredTokens(row);
  const sessions = `${row.sessions} session${row.sessions === 1 ? "" : "s"}`;
  return (
    <div
      className="min-w-44 flex-1"
      title={[
        `${label}: ${total.toLocaleString()} tokens metered`,
        `Input ${row.inputTokens.toLocaleString()} · output ${row.outputTokens.toLocaleString()} · cache writes ${row.cacheWriteTokens.toLocaleString()}`,
        `Cache reads ${row.cacheReadTokens.toLocaleString()} (not in the bar)`,
      ].join("\n")}
    >
      <div className="flex items-baseline justify-between gap-2">
        <span className="truncate text-xs text-zinc-200">{label}</span>
        <span className="shrink-0 font-mono text-[11px] text-zinc-300">{formatTokens(total)} tokens</span>
      </div>
      <div
        role="meter"
        aria-label={`${label} tokens`}
        aria-valuemin={0}
        aria-valuemax={Math.max(peak, total)}
        aria-valuenow={total}
        className="mt-1 h-1.5 overflow-hidden rounded-full bg-zinc-800"
      >
        <div className={`h-full rounded-full ${PROVIDER_ACCENT[row.provider]}`} style={{ width: `${meterPercent(total, peak)}%` }} />
      </div>
      <div className="mt-1 text-[10px] text-zinc-500">
        {sessions} · {formatTokens(row.cacheReadTokens)} cache read
      </div>
    </div>
  );
}
