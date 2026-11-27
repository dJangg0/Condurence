import { PROVIDER_LABELS, type ProviderUsage } from "../types/orchestration";
import { formatTokens, meterPercent, meteredTokens } from "../lib/usage";
import { PlanLimitBars } from "./PlanLimitBars";
import { ProviderIcon } from "./ProviderIcon";
import { PROVIDER_ACCENT } from "./providerLogos";

/**
 * Floating card for the topology pane. One row per provider the run used, each with its badge,
 * tokens relative to the busiest provider, sessions and cache reads. It is display only, so it
 * lets clicks through to the canvas beneath it.
 */
export function UsageMeter({ rows, live }: { rows: ProviderUsage[]; live: boolean }) {
  const peak = Math.max(0, ...rows.map(meteredTokens));
  return (
    <section
      aria-label="Provider usage"
      className="w-60 rounded-xl border border-zinc-700/80 bg-zinc-900/90 p-3 shadow-xl shadow-black/40 backdrop-blur-sm"
    >
      <header className="flex items-baseline justify-between gap-2">
        <h2 className="text-[11px] font-semibold uppercase tracking-wider text-zinc-400">Usage</h2>
        <span className="text-[10px] text-zinc-500">{live ? "This run" : "Last run"}</span>
      </header>
      <ul className="mt-2.5 space-y-3">
        {rows.map((row) => (
          <li key={row.provider}>
            <ProviderMeter row={row} peak={peak} />
          </li>
        ))}
      </ul>
    </section>
  );
}

function ProviderMeter({ row, peak }: { row: ProviderUsage; peak: number }) {
  const label = PROVIDER_LABELS[row.provider];

  if (!row.measured) {
    return (
      <div className="flex items-center gap-2.5" title={`${label} does not expose token usage to Condurence yet.`}>
        <ProviderIcon provider={row.provider} />
        <div className="min-w-0 flex-1">
          <div className="truncate text-xs text-zinc-300">{label}</div>
          <div className="text-[11px] text-zinc-500">Not measured yet</div>
        </div>
      </div>
    );
  }

  const total = meteredTokens(row);
  const sessions = `${row.sessions} session${row.sessions === 1 ? "" : "s"}`;
  return (
    <div
      className="flex items-center gap-2.5"
      title={[
        `${label}: ${total.toLocaleString()} tokens metered`,
        `Input ${row.inputTokens.toLocaleString()} · output ${row.outputTokens.toLocaleString()} · cache writes ${row.cacheWriteTokens.toLocaleString()}`,
        `Cache reads ${row.cacheReadTokens.toLocaleString()} (not in the bar)`,
      ].join("\n")}
    >
      <ProviderIcon provider={row.provider} />
      <div className="min-w-0 flex-1">
        <div className="flex items-baseline justify-between gap-2">
          <span className="truncate text-xs font-medium text-zinc-100">{label}</span>
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
        {row.planLimit ? <PlanLimitBars limit={row.planLimit} nowMs={Date.now()} /> : null}
      </div>
    </div>
  );
}
