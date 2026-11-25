import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { CondurenceMark } from "./CondurenceMark";
import { SplashScreen } from "./SplashScreen";

describe("CondurenceMark animation", () => {
  it("stays static unless animate is set", () => {
    const markup = renderToStaticMarkup(<CondurenceMark />);
    expect(markup).not.toContain("oc-splash");
    expect(markup).not.toContain("pathLength");
  });

  it("draws each track in its own time and lands its node when that track finishes", () => {
    const markup = renderToStaticMarkup(<CondurenceMark animate />);
    const durations = [...markup.matchAll(/oc-splash-track[^>]*animation-duration:([\d.]+)ms/g)].map((m) =>
      Number(m[1]),
    );
    const delays = [...markup.matchAll(/oc-splash-node[^>]*animation-delay:([\d.]+)ms/g)].map((m) => Number(m[1]));

    expect(durations).toHaveLength(3);
    expect(delays).toEqual(durations);
    // Outer track has the longest sweep, so it draws slowest and lands last.
    expect(durations[0]).toBeGreaterThan(durations[1]);
    expect(durations[1]).toBeGreaterThan(durations[2]);
  });
});

describe("SplashScreen", () => {
  it("renders the mark and the name as a status region", () => {
    const markup = renderToStaticMarkup(<SplashScreen />);
    expect(markup).toContain('role="status"');
    expect(markup).toContain("Starting Condurence");
    expect(markup).toContain("Condurence</div>");
    expect(markup).toContain("oc-splash-rise");
  });
});
