import { describe, expect, it } from "vitest";
import { EARLIER_STORAGE_KEY, STORAGE_KEY, parsePreferences, storedPreferences } from "./preferences";

const store = (entries: Record<string, string>) => (key: string) => entries[key] ?? null;

describe("storedPreferences", () => {
  it("reads the current key", () => {
    const read = store({ [STORAGE_KEY]: JSON.stringify({ theme: "light", displayName: "Ada" }) });
    expect(storedPreferences(read)).toEqual({ theme: "light", displayName: "Ada" });
  });

  it("carries over what was saved under the app's earlier name", () => {
    const read = store({ [EARLIER_STORAGE_KEY]: JSON.stringify({ theme: "system", displayName: "Ada" }) });
    expect(storedPreferences(read)).toEqual({ theme: "system", displayName: "Ada" });
  });

  it("prefers the current key when both exist", () => {
    const read = store({
      [STORAGE_KEY]: JSON.stringify({ theme: "light", displayName: "New" }),
      [EARLIER_STORAGE_KEY]: JSON.stringify({ theme: "dark", displayName: "Old" }),
    });
    expect(storedPreferences(read).displayName).toBe("New");
  });

  it("uses the defaults when nothing is stored", () => {
    expect(storedPreferences(store({}))).toEqual({ theme: "dark", displayName: "" });
  });

  it("uses the defaults for damaged text and keeps only valid fields", () => {
    expect(parsePreferences("{not json")).toEqual({ theme: "dark", displayName: "" });
    expect(parsePreferences(JSON.stringify({ theme: "neon", displayName: 7 }))).toEqual({ theme: "dark", displayName: "" });
  });
});
