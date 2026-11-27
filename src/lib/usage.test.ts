import { describe, expect, it } from "vitest";
import { PROVIDER_MONOGRAM, formatAge, formatTokens, meterPercent, meteredTokens, formatReset, planLabel, planWindows, windowLabel, windowState } from "./usage";
import type { ProviderUsage } from "../types/orchestration";

const usage = (overrides: Partial<ProviderUsage> = {}): ProviderUsage => ({
  provider: "claude",
  measured: true,
  sessions: 1,
  inputTokens: 0,
  outputTokens: 0,
  cacheReadTokens: 0,
  cacheWriteTokens: 0,
  ...overrides,
});

describe("meteredTokens", () => {
  it("counts fresh input, output and cache writes but not cache reads", () => {
    const row = usage({ inputTokens: 100, outputTokens: 20, cacheWriteTokens: 5, cacheReadTokens: 9_000_000 });
    expect(meteredTokens(row)).toBe(125);
  });
});

describe("meterPercent", () => {
  it("scales against the busiest provider and never exceeds 100", () => {
    expect(meterPercent(50, 200)).toBe(25);
    expect(meterPercent(200, 200)).toBe(100);
    expect(meterPercent(300, 200)).toBe(100);
  });

  it("is zero when there is nothing to show", () => {
    expect(meterPercent(0, 0)).toBe(0);
    expect(meterPercent(0, 500)).toBe(0);
    expect(meterPercent(10, 0)).toBe(0);
  });
});

describe("formatTokens", () => {
  it("keeps small counts exact", () => {
    expect(formatTokens(0)).toBe("0");
    expect(formatTokens(980)).toBe("980");
  });

  it("uses one decimal for small thousands and whole numbers above that", () => {
    expect(formatTokens(1_200)).toBe("1.2k");
    expect(formatTokens(12_345)).toBe("12k");
  });

  it("uses millions and carries rounding into the next unit", () => {
    expect(formatTokens(1_234_567)).toBe("1.2M");
    expect(formatTokens(12_345_678)).toBe("12M");
    expect(formatTokens(999_960)).toBe("1M");
  });

  it("treats negative and fractional input safely", () => {
    expect(formatTokens(-4)).toBe("0");
    expect(formatTokens(1_499.6)).toBe("1.5k");
  });
});

describe("PROVIDER_MONOGRAM", () => {
  it("gives every provider a distinct badge, with the profile pane's letters for the shared ones", () => {
    const letters = Object.values(PROVIDER_MONOGRAM);
    expect(new Set(letters).size).toBe(letters.length);
    expect(PROVIDER_MONOGRAM).toMatchObject({ claude: "C", codex: "G", opencode: "O" });
  });
});

describe("plan limits", () => {
  const NOW = Date.UTC(2026, 10, 26, 12, 0, 0);
  const inDays = (days: number) => Math.floor(NOW / 1000) + days * 86_400;

  it("names the plan and each window by its length", () => {
    expect(planLabel("free")).toBe("Free");
    expect(planLabel(null)).toBe("Plan");
    expect(windowLabel(300)).toBe("5-hour");
    expect(windowLabel(10_080)).toBe("weekly");
    expect(windowLabel(43_200)).toBe("30-day");
    expect(windowLabel(30)).toBe("30-minute");
    expect(windowLabel(null)).toBe("usage");
  });

  it("reads a window as used until it resets, then as fresh", () => {
    const window = { usedPercent: 9, windowMinutes: 43_200, resetsAt: inDays(20) };
    expect(windowState(window, NOW)).toEqual({ percent: 9, reset: false, tone: "ok" });
    expect(windowState({ ...window, usedPercent: 85 }, NOW).tone).toBe("warn");
    expect(windowState({ ...window, usedPercent: 130 }, NOW)).toEqual({ percent: 100, reset: false, tone: "full" });
    expect(windowState({ ...window, resetsAt: inDays(-1) }, NOW)).toEqual({ percent: 0, reset: true, tone: "ok" });
    expect(windowState({ ...window, resetsAt: null }, NOW).percent).toBe(9);
  });

  it("says when a window resets, in the largest fitting unit", () => {
    expect(formatReset(inDays(12), NOW)).toBe("resets in 12 d");
    expect(formatReset(Math.floor(NOW / 1000) + 3 * 3600, NOW)).toBe("resets in 3 h");
    expect(formatReset(Math.floor(NOW / 1000) + 20 * 60, NOW)).toBe("resets in 20 min");
    expect(formatReset(inDays(-1), NOW)).toBe("reset");
    expect(formatReset(null, NOW)).toBeNull();
  });

  it("lists only the windows the plan reported", () => {
    const primary = { usedPercent: 1, windowMinutes: 300, resetsAt: null };
    expect(planWindows({ plan: "plus", primary, secondary: null, reached: false, reportedMs: 1 })).toEqual([primary]);
    expect(planWindows({ plan: null, primary: null, secondary: null, reached: false, reportedMs: 1 })).toEqual([]);
  });

  it("notes how old a report is only once it is no longer fresh", () => {
    expect(formatAge(NOW - 5 * 60_000, NOW)).toBeNull();
    expect(formatAge(NOW - 25 * 60_000, NOW)).toBe("updated 25 min ago");
    expect(formatAge(NOW - 3 * 3_600_000, NOW)).toBe("updated 3 h ago");
    expect(formatAge(NOW - 2 * 86_400_000, NOW)).toBe("updated 2 d ago");
    expect(formatAge(0, NOW)).toBeNull();
  });
});
