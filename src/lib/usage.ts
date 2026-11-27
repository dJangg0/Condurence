import type { AgentProvider, PlanLimit, PlanWindow, ProviderUsage } from "../types/orchestration";

/** Badge letters for each provider. Claude, Codex and OpenCode match the account rows in the profile pane. */
export const PROVIDER_MONOGRAM: Record<AgentProvider, string> = {
  claude: "C",
  codex: "G",
  opencode: "O",
  gemini: "Gm",
};

/**
 * Tokens the meter measures: fresh input, output and cache writes. Cache reads are left out of the
 * bar and shown beside it, because an agent re-reads its whole context on every turn and that
 * count would dwarf everything else.
 */
export function meteredTokens(usage: ProviderUsage): number {
  return usage.inputTokens + usage.outputTokens + usage.cacheWriteTokens;
}

/** Bar length for `value` against the busiest provider's `peak`, from 0 to 100. */
export function meterPercent(value: number, peak: number): number {
  if (value <= 0 || peak <= 0) return 0;
  return Math.min(100, Math.round((value / peak) * 100));
}

/** Compact token count: `980`, `12k`, `1.2M`. */
export function formatTokens(count: number): string {
  const n = Math.max(0, Math.round(count));
  if (n < 1_000) return String(n);
  const [scale, suffix] = n < 1_000_000 ? [1_000, "k"] : [1_000_000, "M"];
  const value = n / scale;
  const text = value.toFixed(value < 10 ? 1 : 0);
  // Rounding can carry 999.96k up to 1000k. Show that as the next unit instead.
  return text === "1000" && suffix === "k" ? "1M" : `${text}${suffix}`;
}

/** The plan name as shown to people: `free` becomes `Free`. */
export function planLabel(plan: string | null): string {
  const text = (plan ?? "").trim();
  return text ? text.charAt(0).toUpperCase() + text.slice(1).toLowerCase() : "Plan";
}

/** Names a window by its length: `5-hour`, `weekly`, `30-day`. */
export function windowLabel(minutes: number | null): string {
  if (!minutes || minutes <= 0) return "usage";
  if (minutes === 10_080) return "weekly";
  if (minutes < 60) return `${minutes}-minute`;
  if (minutes < 1_440) return `${Math.round(minutes / 60)}-hour`;
  return `${Math.round(minutes / 1_440)}-day`;
}

export type WindowState = {
  /** Percent shown, 0 to 100. Zero once the window has reset. */
  percent: number;
  /** True when the window reset after the last report, so the report no longer applies. */
  reset: boolean;
  tone: "ok" | "warn" | "full";
};

/** What a window reads as at `nowMs`: its percent, or a fresh window if it has since reset. */
export function windowState(window: PlanWindow, nowMs: number): WindowState {
  const reset = window.resetsAt !== null && window.resetsAt * 1000 <= nowMs;
  const percent = reset ? 0 : Math.max(0, Math.min(100, window.usedPercent));
  return { percent, reset, tone: percent >= 100 ? "full" : percent >= 80 ? "warn" : "ok" };
}

/** `resets in 12 d`, `resets in 3 h`, `resets in 20 min`, or `reset` once it has passed. */
export function formatReset(resetsAt: number | null, nowMs: number): string | null {
  if (resetsAt === null) return null;
  const minutes = Math.ceil((resetsAt * 1000 - nowMs) / 60_000);
  if (minutes <= 0) return "reset";
  if (minutes < 60) return `resets in ${minutes} min`;
  if (minutes < 1_440) return `resets in ${Math.round(minutes / 60)} h`;
  return `resets in ${Math.round(minutes / 1_440)} d`;
}

/** The windows a plan limit reports, primary first. */
export function planWindows(limit: PlanLimit): PlanWindow[] {
  return [limit.primary, limit.secondary].filter((window): window is PlanWindow => window !== null);
}

/** How old a plan report is, such as `updated 2 h ago`. `null` while it is fresh (under 10 minutes). */
export function formatAge(reportedMs: number, nowMs: number): string | null {
  const minutes = Math.floor((nowMs - reportedMs) / 60_000);
  if (reportedMs <= 0 || minutes < 10) return null;
  if (minutes < 60) return `updated ${minutes} min ago`;
  if (minutes < 1_440) return `updated ${Math.round(minutes / 60)} h ago`;
  return `updated ${Math.round(minutes / 1_440)} d ago`;
}
