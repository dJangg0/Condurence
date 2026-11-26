import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { UsageMeter } from "./UsageMeter";
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

/** Width percentage of the bar belonging to `label`, read back from the rendered markup. */
function barWidth(markup: string, label: string): string | undefined {
  const pattern = new RegExp(`aria-label="${label} tokens"[\\s\\S]*?width:([\\d.]+)%`);
  return pattern.exec(markup)?.[1];
}

describe("UsageMeter", () => {
  it("scales each bar against the busiest provider", () => {
    const markup = renderToStaticMarkup(
      <UsageMeter
        live
        rows={[
          row({ provider: "claude", inputTokens: 1_000_000, outputTokens: 0 }),
          row({ provider: "codex", inputTokens: 250_000, outputTokens: 250_000 }),
        ]}
      />,
    );
    expect(barWidth(markup, "Claude Code")).toBe("100");
    expect(barWidth(markup, "Codex CLI")).toBe("50");
    expect(markup).toContain("1.0M tokens");
    expect(markup).toContain("500k tokens");
  });

  it("labels the strip as this run while live, and as the last run once it has ended", () => {
    const rows = [row({ inputTokens: 10 })];
    expect(renderToStaticMarkup(<UsageMeter rows={rows} live />)).toContain("This run");
    expect(renderToStaticMarkup(<UsageMeter rows={rows} live={false} />)).toContain("Last run");
  });

  it("shows providers without a reader as not measured rather than zero", () => {
    const markup = renderToStaticMarkup(
      <UsageMeter live rows={[row({ provider: "gemini", measured: false, sessions: 0 })]} />,
    );
    expect(markup).toContain("Gemini CLI");
    expect(markup).toContain("Not measured yet");
    expect(markup).not.toContain("tokens</span>");
  });

  it("reports cache reads beside the bar instead of inside it", () => {
    const markup = renderToStaticMarkup(
      <UsageMeter live rows={[row({ inputTokens: 1_000, cacheReadTokens: 2_500_000 })]} />,
    );
    expect(markup).toContain("1.0k tokens");
    expect(markup).toContain("2.5M cache read");
    expect(barWidth(markup, "Claude Code")).toBe("100");
  });

  it("renders a zero-width bar when nothing has been used yet", () => {
    const markup = renderToStaticMarkup(<UsageMeter live rows={[row({})]} />);
    expect(barWidth(markup, "Claude Code")).toBe("0");
    expect(markup).toContain("0 tokens");
  });
});
