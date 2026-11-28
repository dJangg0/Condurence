import { beforeEach, describe, expect, it, vi } from "vitest";

// xterm.js needs a browser. These fakes hold just the calls the store makes, so the tests exercise the
// store's own rules: which terminal is shown where, what is replayed, and when a terminal is released.
const fakes = vi.hoisted(() => {
  class FakeElement {
    parent: FakeElement | null = null;
    children: FakeElement[] = [];
    /** A container is shown when it has a layout box. */
    shown = true;

    appendChild(child: FakeElement): FakeElement {
      child.remove();
      child.parent = this;
      this.children.push(child);
      return child;
    }

    remove(): void {
      if (this.parent) this.parent.children = this.parent.children.filter((child) => child !== this);
      this.parent = null;
    }

    getClientRects(): unknown[] {
      return this.shown ? [{}] : [];
    }
  }

  class FakeFit {
    terminal: FakeTerminal | undefined;

    fit(): void {
      // Fitting gives the terminal the size of its container.
      this.terminal?.resize(100, 40);
    }
  }

  class FakeTerminal {
    static instances: FakeTerminal[] = [];
    element: FakeElement | undefined;
    cols = 80;
    rows = 24;
    options: Record<string, unknown>;
    written: string[] = [];
    opened: FakeElement[] = [];
    disposed = false;
    private dataListeners = new Set<(data: string) => void>();
    private resizeListeners = new Set<(size: { cols: number; rows: number }) => void>();

    constructor(options: Record<string, unknown>) {
      this.options = { ...options };
      FakeTerminal.instances.push(this);
    }

    loadAddon(addon: FakeFit): void {
      addon.terminal = this;
    }

    open(parent: FakeElement): void {
      this.opened.push(parent);
      this.element = new FakeElement();
      parent.appendChild(this.element);
    }

    write(data: string): void {
      this.written.push(data);
    }

    refresh(): void {}

    dispose(): void {
      this.disposed = true;
    }

    onData(listener: (data: string) => void): { dispose(): void } {
      this.dataListeners.add(listener);
      return { dispose: () => void this.dataListeners.delete(listener) };
    }

    onResize(listener: (size: { cols: number; rows: number }) => void): { dispose(): void } {
      this.resizeListeners.add(listener);
      return { dispose: () => void this.resizeListeners.delete(listener) };
    }

    /** Keystrokes typed into the terminal. */
    type(data: string): void {
      for (const listener of this.dataListeners) listener(data);
    }

    /** A size change, as fitting makes it, reported to the listeners. */
    resize(cols: number, rows: number): void {
      this.cols = cols;
      this.rows = rows;
      for (const listener of this.resizeListeners) listener({ cols, rows });
    }
  }

  return { FakeElement, FakeFit, FakeTerminal };
});

const backend = vi.hoisted(() => ({
  outputs: new Map<string, (chunk: string) => void>(),
  exits: new Map<string, (code: number) => void>(),
  getPtyHistory: vi.fn(async (_sessionId: string) => ""),
  writeAgentPty: vi.fn(async (_sessionId: string, _data: string) => undefined),
  resizeAgentPty: vi.fn(async (_sessionId: string, _cols: number, _rows: number) => undefined),
}));

vi.mock("@xterm/xterm", () => ({ Terminal: fakes.FakeTerminal }));
vi.mock("@xterm/addon-fit", () => ({ FitAddon: fakes.FakeFit }));
vi.mock("./ipc", () => ({
  events: {
    ptyOutput: async (sessionId: string, onChunk: (chunk: string) => void) => {
      backend.outputs.set(sessionId, onChunk);
      return () => void backend.outputs.delete(sessionId);
    },
    ptyExit: async (sessionId: string, onExit: (code: number) => void) => {
      backend.exits.set(sessionId, onExit);
      return () => void backend.exits.delete(sessionId);
    },
  },
  ipc: {
    getPtyHistory: backend.getPtyHistory,
    writeAgentPty: backend.writeAgentPty,
    resizeAgentPty: backend.resizeAgentPty,
  },
}));

type Store = typeof import("./terminalStore");
let store: Store;

const flush = () => new Promise<void>((resolve) => setTimeout(resolve, 0));

const pane = (shown = true): InstanceType<typeof fakes.FakeElement> => {
  const element = new fakes.FakeElement();
  element.shown = shown;
  return element;
};

const attach = (
  sessionId: string,
  container: InstanceType<typeof fakes.FakeElement>,
  interactive = true,
  onExit: (code: number) => void = () => undefined,
) => store.attachTerminal(sessionId, container as unknown as HTMLElement, { interactive, onExit });

const terminal = (index = 0): InstanceType<typeof fakes.FakeTerminal> => fakes.FakeTerminal.instances[index];

beforeEach(async () => {
  vi.resetModules();
  vi.clearAllMocks();
  backend.outputs.clear();
  backend.exits.clear();
  fakes.FakeTerminal.instances = [];
  store = await import("./terminalStore");
});

describe("terminal store", () => {
  it("moves a session's terminal to its next container instead of rebuilding it", async () => {
    const first = pane();
    const before = attach("s1", first);
    await flush();
    before.detach();

    const second = pane();
    const after = attach("s1", second);
    await flush();

    expect(fakes.FakeTerminal.instances).toHaveLength(1);
    const term = terminal();
    expect(term.opened).toHaveLength(1);
    expect(term.opened[0]).toBe(first);
    expect(second.children[0]).toBe(term.element);
    expect(first.children).toHaveLength(0);
    expect(backend.getPtyHistory).toHaveBeenCalledTimes(1);
    expect(after.exitCode).toBeNull();
  });

  it("keeps receiving a session's output while no view shows it", async () => {
    const attached = attach("s1", pane());
    await flush();
    attached.detach();

    backend.outputs.get("s1")?.("still building\r\n");

    expect(terminal().written).toContain("still building\r\n");
  });

  it("replays history first, then the output that arrived while it loaded, then live output", async () => {
    let finishHistory: (text: string) => void = () => undefined;
    backend.getPtyHistory.mockImplementationOnce(
      () =>
        new Promise<string>((resolve) => {
          finishHistory = resolve;
        }),
    );
    attach("s1", pane());
    await flush();

    backend.outputs.get("s1")?.("early ");
    finishHistory("history ");
    await flush();
    backend.outputs.get("s1")?.("late");

    expect(terminal().written).toEqual(["history ", "early ", "late"]);
  });

  it("forwards keystrokes only while the terminal is interactive", async () => {
    attach("s1", pane(), false);
    await flush();
    const term = terminal();

    term.type("ls\r");
    expect(backend.writeAgentPty).not.toHaveBeenCalled();
    expect(term.options.disableStdin).toBe(true);

    store.setTerminalInteractive("s1", true);
    term.type("ls\r");
    expect(backend.writeAgentPty).toHaveBeenCalledWith("s1", "ls\r");
    expect(term.options.disableStdin).toBe(false);

    store.setTerminalInteractive("s1", false);
    term.type("pwd\r");
    expect(backend.writeAgentPty).toHaveBeenCalledTimes(1);
  });

  it("sizes the process only once the terminal is shown", async () => {
    const container = pane(false);
    const attached = attach("s1", container);
    await flush();
    expect(backend.resizeAgentPty).not.toHaveBeenCalled();

    container.shown = true;
    attached.refit();
    await flush();

    expect(backend.resizeAgentPty).toHaveBeenLastCalledWith("s1", 100, 40);
    expect(backend.resizeAgentPty).not.toHaveBeenCalledWith("s1", 80, 24);
  });

  it("releases an ended session's terminal and stops its output", async () => {
    const attached = attach("s1", pane());
    await flush();

    store.disposeTerminal("s1");
    expect(terminal().disposed).toBe(true);
    expect(backend.outputs.has("s1")).toBe(false);

    // A view still holding the old terminal unmounting late does nothing harmful.
    attached.detach();
    attach("s1", pane());
    expect(fakes.FakeTerminal.instances).toHaveLength(2);
  });

  it("keeps an exited session's terminal while no view shows it, up to a limit", async () => {
    const exitAndHide = async (sessionId: string) => {
      const attached = attach(sessionId, pane());
      await flush();
      backend.exits.get(sessionId)?.(0);
      attached.detach();
    };

    for (let i = 0; i < 16; i++) await exitAndHide(`done-${i}`);
    expect(fakes.FakeTerminal.instances.some((term) => term.disposed)).toBe(false);

    await exitAndHide("done-16");
    const released = fakes.FakeTerminal.instances.filter((term) => term.disposed);
    expect(released).toHaveLength(1);
    expect(released[0]).toBe(terminal(0));
  });

  it("writes the exit line and reports the exit code to the view that shows it", async () => {
    const onExit = vi.fn();
    const attached = attach("s1", pane(), true, onExit);
    await flush();

    backend.exits.get("s1")?.(3);

    expect(onExit).toHaveBeenCalledWith(3);
    expect(attached.exitCode).toBe(3);
    expect(terminal().written.join("")).toContain("process exited with code 3");
  });

  it("does not report an exit to a view that has already let go", async () => {
    const onExit = vi.fn();
    const attached = attach("s1", pane(), true, onExit);
    await flush();
    attached.detach();

    backend.exits.get("s1")?.(0);

    expect(onExit).not.toHaveBeenCalled();
  });

  it("leaves the terminal with the view that took it last when an older view lets go", async () => {
    const older = pane();
    const newer = pane();
    const first = attach("s1", older);
    await flush();
    const second = attach("s1", newer);

    first.refit();
    first.detach();

    expect(newer.children[0]).toBe(terminal().element);
    expect(second.exitCode).toBeNull();
  });
});
