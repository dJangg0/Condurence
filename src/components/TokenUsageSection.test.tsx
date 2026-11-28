import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { TokenUsageSection, USAGE_SUMMARY_DAYS } from "./TokenUsageSection";
import type { ProviderUsage } from "../types/orchestration";

const row = (overrides: Partial<ProviderUsage>): ProviderUsage => ({
  provider: "claude",
  measured: true,
  sessions: 2,
  inputTokens: 0,
  outputTokens: 0,
  cacheReadTokens: 0,
  cacheWriteTokens: 0,
  ...overrides,
});

/** Width percentage of the bar that belongs to `label`, read back from the rendered markup. */
function barWidth(markup: string, label: string): string | undefined {
  const pattern = new RegExp(`aria-label="${label} tokens[^"]*"[\\s\\S]*?width:([\\d.]+)%`);
  return pattern.exec(markup)?.[1];
}

describe("TokenUsageSection", () => {
  it("shows a loading note until the logs have been read", () => {
    const markup = renderToStaticMarkup(<TokenUsageSection rows={null} error={null} />);
    expect(markup).toContain("Reading session logs");
    expect(markup).not.toContain("tokens</span>");
  });

  it("shows the read error instead of numbers", () => {
    const markup = renderToStaticMarkup(<TokenUsageSection rows={null} error="home directory missing" />);
    expect(markup).toContain("home directory missing");
    expect(markup).not.toContain("Reading session logs");
  });

  it("gives each measured provider its token count, session count and a bar scaled to the busiest", () => {
    const markup = renderToStaticMarkup(
      <TokenUsageSection
        error={null}
        rows={[
          row({ provider: "claude", sessions: 3, inputTokens: 2_000_000, outputTokens: 0, cacheReadTokens: 90_000_000 }),
          row({ provider: "codex", sessions: 1, inputTokens: 500_000, outputTokens: 500_000 }),
        ]}
      />,
    );
    expect(markup).toContain("Claude Code");
    expect(markup).toContain("2.0M tokens");
    expect(markup).toContain("3 sessions · 90M cache read");
    expect(barWidth(markup, "Claude Code")).toBe("100");

    expect(markup).toContain("Codex CLI");
    expect(markup).toContain("1 session ");
    expect(barWidth(markup, "Codex CLI")).toBe("50");
  });

  it("labels providers the logs cannot be read for as not measured", () => {
    const markup = renderToStaticMarkup(
      <TokenUsageSection error={null} rows={[row({ provider: "gemini", measured: false, sessions: 0 })]} />,
    );
    expect(markup).toContain("Antigravity CLI");
    expect(markup).toContain("Not measured yet");
    expect(markup).not.toContain("tokens</span>");
  });

  it("says so when no CLI session started in the window", () => {
    const markup = renderToStaticMarkup(
      <TokenUsageSection error={null} rows={[row({ sessions: 0 }), row({ provider: "codex", sessions: 0 })]} />,
    );
    expect(markup).toContain(`No CLI sessions in the last ${USAGE_SUMMARY_DAYS} days.`);
  });

  it("does not claim a summary when there are sessions", () => {
    const markup = renderToStaticMarkup(
      <TokenUsageSection error={null} rows={[row({ sessions: 1, inputTokens: 10 })]} />,
    );
    expect(markup).not.toContain("No CLI sessions");
  });

  it("shows the ChatGPT plan's own usage meter under Codex", () => {
    const farFuture = 4_102_444_800; // 2100-01-01, so the window has not reset
    const markup = renderToStaticMarkup(
      <TokenUsageSection
        error={null}
        rows={[
          row({
            provider: "codex",
            sessions: 4,
            inputTokens: 1000,
            planLimit: {
              plan: "free",
              primary: { usedPercent: 9, windowMinutes: 43_200, resetsAt: farFuture },
              secondary: null,
              reached: false,
              reportedMs: 1,
            },
          }),
          row({ provider: "claude", sessions: 1, inputTokens: 10 }),
        ]}
      />,
    );
    expect(markup).toContain('aria-label="Free plan, 30-day limit"');
    expect(markup).toContain("Free plan · 30-day");
    expect(markup).toContain("9% used");
    expect(markup).toMatch(/aria-valuenow="9"/);
    expect(markup).not.toContain("Limit reached");
    // Providers without a plan report get no plan meter.
    expect(markup.match(/plan limits/g)?.length).toBe(1);
  });

  it("flags a reached limit and shows a window that has since reset as reset", () => {
    const markup = renderToStaticMarkup(
      <TokenUsageSection
        error={null}
        rows={[
          row({
            provider: "codex",
            sessions: 1,
            inputTokens: 5,
            planLimit: {
              plan: "plus",
              primary: { usedPercent: 100, windowMinutes: 300, resetsAt: 1 },
              secondary: { usedPercent: 100, windowMinutes: 10_080, resetsAt: 4_102_444_800 },
              reached: true,
              reportedMs: 1,
            },
          }),
        ]}
      />,
    );
    expect(markup).toContain("Plus plan · 5-hour");
    expect(markup).toContain(">reset<");
    expect(markup).toContain("Plus plan · weekly");
    expect(markup).toContain("100% used");
    expect(markup).toContain("Limit reached");
  });

  it("shows the Claude subscription meter with its five-hour and weekly windows and how old the report is", () => {
    const farFuture = 4_102_444_800;
    const markup = renderToStaticMarkup(
      <TokenUsageSection
        error={null}
        rows={[
          row({
            provider: "claude",
            sessions: 2,
            inputTokens: 100,
            planLimit: {
              plan: "pro",
              primary: { usedPercent: 68, windowMinutes: 300, resetsAt: farFuture },
              secondary: { usedPercent: 55, windowMinutes: 10_080, resetsAt: farFuture },
              reached: false,
              reportedMs: Date.now() - 3 * 3_600_000,
            },
          }),
        ]}
      />,
    );
    expect(markup).toContain("Pro plan · 5-hour");
    expect(markup).toContain("68% used");
    expect(markup).toContain("Pro plan · weekly");
    expect(markup).toContain("55% used");
    expect(markup).toContain("updated 3 h ago");
  });
});
