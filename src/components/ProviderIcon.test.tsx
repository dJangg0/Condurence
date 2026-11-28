import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";

vi.mock("./providerLogos", async (importOriginal) => {
  const original = await importOriginal<typeof import("./providerLogos")>();
  const { codex: _withoutLogo, ...logos } = original.PROVIDER_LOGO;
  return { ...original, PROVIDER_LOGO: logos };
});

import { ProviderIcon } from "./ProviderIcon";

describe("ProviderIcon", () => {
  it("draws the provider's logo, hidden from assistive technology", () => {
    const markup = renderToStaticMarkup(<ProviderIcon provider="claude" />);
    expect(markup).toMatch(/aria-hidden="true"/);
    expect(markup).toContain('fill="#D97757"');
  });

  it("falls back to the provider letter on the accent colour when a logo is missing", () => {
    const markup = renderToStaticMarkup(<ProviderIcon provider="codex" />);
    expect(markup).not.toContain("<svg");
    expect(markup).toContain("bg-emerald-400");
    expect(markup).toMatch(/>G</);
  });

  it("has a smaller size for inline use", () => {
    expect(renderToStaticMarkup(<ProviderIcon provider="claude" size="sm" />)).toContain("h-5 w-5");
    expect(renderToStaticMarkup(<ProviderIcon provider="claude" />)).toContain("h-8 w-8");
  });
});
