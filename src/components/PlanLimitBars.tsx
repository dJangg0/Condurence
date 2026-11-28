import type { PlanLimit } from "../types/orchestration";
import { formatAge, formatReset, planLabel, planWindows, windowLabel, windowState } from "../lib/usage";

const TONE_BAR = { ok: "bg-emerald-400", warn: "bg-amber-400", full: "bg-red-500" } as const;

/**
 * A subscription's own usage meter, read from the CLI's logs: one bar per rate-limit window, such
 * as the ChatGPT Free allowance used so far. `nowMs` is passed in so the reading is testable.
 */
export function PlanLimitBars({ limit, nowMs }: { limit: PlanLimit; nowMs: number }) {
  const windows = planWindows(limit);
  if (windows.length === 0) return null;
  return (
    <div className="mt-1.5 space-y-1" aria-label={`${planLabel(limit.plan)} plan limits`}>
      {windows.map((window, index) => {
        const state = windowState(window, nowMs);
        const label = `${planLabel(limit.plan)} plan, ${windowLabel(window.windowMinutes)} limit`;
        const reset = formatReset(window.resetsAt, nowMs);
        return (
          <div key={index}>
            <div className="flex items-baseline justify-between gap-2 text-[10px] text-zinc-400">
              <span className="truncate">
                {planLabel(limit.plan)} plan · {windowLabel(window.windowMinutes)}
              </span>
              <span className="shrink-0 font-mono text-zinc-300">
                {state.reset ? "reset" : `${Math.round(state.percent)}% used`}
              </span>
            </div>
            <div
              role="meter"
              aria-label={label}
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={Math.round(state.percent)}
              className="mt-0.5 h-1 overflow-hidden rounded-full bg-zinc-800"
            >
              <div className={`h-full rounded-full ${TONE_BAR[state.tone]}`} style={{ width: `${state.percent}%` }} />
            </div>
            {reset && !state.reset ? <div className="mt-0.5 text-[10px] text-zinc-500">{reset}</div> : null}
          </div>
        );
      })}
      {formatAge(limit.reportedMs, nowMs) ? (
        <div className="text-[10px] text-zinc-500" title="The CLI's last report. It refreshes when you use the CLI.">
          {formatAge(limit.reportedMs, nowMs)}
        </div>
      ) : null}
      {limit.reached ? <div className="text-[10px] font-medium text-red-300">Limit reached</div> : null}
    </div>
  );
}
