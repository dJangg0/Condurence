import type { ProviderUsage } from "../types/orchestration";

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
