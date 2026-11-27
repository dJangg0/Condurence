import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const log: string[] = [];
const dirty = new Map<string, () => void>();
vi.mock("./ipc", () => ({
  ipc: {
    watchWorkspace: vi.fn(async (path: string) => {
      log.push(`watch ${path}`);
      return `id:${path}`;
    }),
    unwatchWorkspace: vi.fn(async (path: string) => void log.push(`unwatch ${path}`)),
  },
  events: {
    workspaceDirty: vi.fn(async (id: string, onDirty: () => void) => {
      dirty.set(id, onDirty);
      return () => void log.push(`stop ${id}`);
    }),
  },
}));

import { watchWorkspaceDirty } from "./workspaceWatch";

const settle = () => vi.advanceTimersByTimeAsync(0);

describe("watchWorkspaceDirty", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    log.length = 0;
    dirty.clear();
  });
  afterEach(() => vi.useRealTimers());

  it("shares one watcher between listeners and tells each of them about a change", async () => {
    const a = vi.fn();
    const b = vi.fn();
    const stopA = watchWorkspaceDirty("/w1", a);
    const stopB = watchWorkspaceDirty("/w1", b);
    await settle();
    expect(log).toEqual(["watch /w1"]);

    dirty.get("id:/w1")?.();
    expect(a).toHaveBeenCalledTimes(1);
    expect(b).toHaveBeenCalledTimes(1);

    stopA();
    dirty.get("id:/w1")?.();
    expect(a).toHaveBeenCalledTimes(1);
    expect(b).toHaveBeenCalledTimes(2);
    stopB();
  });

  it("keeps watching while one listener is left, and stops after the last one has gone", async () => {
    const stopA = watchWorkspaceDirty("/w2", vi.fn());
    const stopB = watchWorkspaceDirty("/w2", vi.fn());
    await settle();

    stopA();
    await vi.advanceTimersByTimeAsync(1000);
    expect(log).toEqual(["watch /w2"]);

    stopB();
    await vi.advanceTimersByTimeAsync(1000);
    expect(log).toEqual(["watch /w2", "stop id:/w2", "unwatch /w2"]);
  });

  it("keeps the watcher when a listener leaves and another arrives straight away", async () => {
    const stop = watchWorkspaceDirty("/w3", vi.fn());
    await settle();
    stop();
    const again = vi.fn();
    const stopAgain = watchWorkspaceDirty("/w3", again);
    await vi.advanceTimersByTimeAsync(1000);
    expect(log).toEqual(["watch /w3"]);
    dirty.get("id:/w3")?.();
    expect(again).toHaveBeenCalledTimes(1);
    stopAgain();
  });

  it("watches again after it was stopped", async () => {
    watchWorkspaceDirty("/w4", vi.fn())();
    await vi.advanceTimersByTimeAsync(1000);
    watchWorkspaceDirty("/w4", vi.fn());
    await settle();
    expect(log.filter((line) => line === "watch /w4")).toHaveLength(2);
  });

  it("watches each workspace on its own", async () => {
    watchWorkspaceDirty("/w5", vi.fn());
    watchWorkspaceDirty("/w6", vi.fn());
    await settle();
    expect(log).toEqual(["watch /w5", "watch /w6"]);
  });
});
