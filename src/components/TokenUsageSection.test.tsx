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
    expect(markup).toContain("Gemini CLI");
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
});
