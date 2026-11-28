import { PROVIDER_LABELS, type ProviderUsage } from "../types/orchestration";
import { formatTokens, meterPercent, meteredTokens } from "../lib/usage";
import { PlanLimitBars } from "./PlanLimitBars";
import { ProviderIcon } from "./ProviderIcon";
import { PROVIDER_ACCENT } from "./providerLogos";

/** Length of the summary, in days. Mirrors `SUMMARY_DAYS` in `src-tauri/src/usage/mod.rs`. */
export const USAGE_SUMMARY_DAYS = 30;

/**
 * Token usage per provider for the profile pane. `rows` is `null` while the session logs are
 * being read. Each bar is that provider's tokens relative to the busiest one.
 */
export function TokenUsageSection({ rows, error }: { rows: ProviderUsage[] | null; error: string | null }) {
  if (error) {
    return (
      <div className="mx-1 rounded-md border border-red-500/30 bg-red-500/10 px-2 py-1.5 text-[11px] text-red-200">{error}</div>
    );
  }
  if (rows === null) {
    return <p className="px-2 py-1.5 text-[11px] text-zinc-500">Reading session logs…</p>;
  }

  const peak = Math.max(0, ...rows.map(meteredTokens));
  const anySessions = rows.some((row) => row.measured && row.sessions > 0);
  return (
    <div>
      <ul className="space-y-0.5">
        {rows.map((row) => (
          <UsageRow key={row.provider} row={row} peak={peak} />
        ))}
      </ul>
      {anySessions ? null : (
        <p className="px-2 pb-1 text-[11px] text-zinc-500">No CLI sessions in the last {USAGE_SUMMARY_DAYS} days.</p>
      )}
      <p className="px-2 pb-1 pt-1.5 text-[10px] leading-relaxed text-zinc-500">
        Read from each CLI&apos;s own session logs on this Mac. Cache reads are listed but not in the bar. Plan limits are the
        CLI&apos;s last report, so they update when you use it. Dollar amounts are not in those logs.
      </p>
    </div>
  );
}

function UsageRow({ row, peak }: { row: ProviderUsage; peak: number }) {
  const label = PROVIDER_LABELS[row.provider];

  if (!row.measured) {
    return (
      <li className="flex items-center justify-between gap-2 px-2 py-1.5" title={`${label} does not expose token usage yet.`}>
        <span className="flex min-w-0 items-center gap-2">
          <ProviderIcon provider={row.provider} size="sm" />
          <span className="truncate text-xs text-zinc-200">{label}</span>
        </span>
        <span className="shrink-0 text-[11px] text-zinc-500">Not measured yet</span>
      </li>
    );
  }

  const total = meteredTokens(row);
  return (
    <li className="px-2 py-1.5">
      <div className="flex items-center justify-between gap-2">
        <span className="flex min-w-0 items-center gap-2">
          <ProviderIcon provider={row.provider} size="sm" />
          <span className="truncate text-xs font-medium text-zinc-100">{label}</span>
        </span>
        <span className="shrink-0 font-mono text-[11px] text-zinc-200">{formatTokens(total)} tokens</span>
      </div>
      <div
        role="meter"
        aria-label={`${label} tokens, last ${USAGE_SUMMARY_DAYS} days`}
        aria-valuemin={0}
        aria-valuemax={Math.max(peak, total)}
        aria-valuenow={total}
        className="mt-1.5 h-1.5 overflow-hidden rounded-full bg-zinc-800"
      >
        <div className={`h-full rounded-full ${PROVIDER_ACCENT[row.provider]}`} style={{ width: `${meterPercent(total, peak)}%` }} />
      </div>
      <div className="mt-1 text-[10px] text-zinc-500">
        {row.sessions} session{row.sessions === 1 ? "" : "s"} · {formatTokens(row.cacheReadTokens)} cache read
      </div>
      {row.planLimit ? <PlanLimitBars limit={row.planLimit} nowMs={Date.now()} /> : null}
    </li>
  );
}
