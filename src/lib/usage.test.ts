import { describe, expect, it } from "vitest";
import { formatTokens, meterPercent, meteredTokens } from "./usage";
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
