import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { events, ipc } from "./ipc";

/**
 * The xterm.js terminal of each open PTY session, kept apart from the view that shows it.
 *
 * A terminal lives as long as its session does, not as long as a view renders it. Switching workspace,
 * opening Settings or the orchestration view unmounts the view that shows a terminal. A terminal rebuilt
 * then is replayed from the backend's byte tail: only the last 256 KB, parsed at xterm's default size before
 * it is fitted, so older output is lost and the screen is drawn wrong. Here a terminal keeps receiving its
 * output while no view shows it, and a view only moves it into its own container.
 */

/** Exited sessions whose terminals are kept while no view shows them. The oldest are released first. */
const FINISHED_RETENTION = 16;

type ExitListener = (exitCode: number) => void;

type Entry = {
  sessionId: string;
  term: Terminal;
  fit: FitAddon;
  /** Whether the terminal has been opened in a container. After that its element is moved, never reopened. */
  opened: boolean;
  /** The container the terminal is shown in, or `null` when no view shows it. */
  container: HTMLElement | null;
  interactive: boolean;
  exitCode: number | null;
  /** The process has not been told this terminal's size yet. Sent on the first fit while shown. */
  sizeUnsent: boolean;
  disposed: boolean;
  unsubscribe: Array<() => void>;
  exitListeners: Set<ExitListener>;
};

const entries = new Map<string, Entry>();
/** Exited sessions that no view shows, oldest first. */
const finished: string[] = [];

export type Attachment = {
  /** The process's exit code once it has ended, otherwise `null`. */
  readonly exitCode: number | null;
  /** Fits the terminal to its container, and tells the process its size, once the container is shown. */
  refit(): void;
  /** Lets go of the container. The terminal keeps running and keeps its output. */
  detach(): void;
};

/**
 * Shows a session's terminal in `container`. The first attachment opens the terminal. Later ones move the
 * same terminal, with its output and scrollback, and nothing is replayed. A session is shown in one
 * container at a time: the latest attachment wins.
 */
export function attachTerminal(
  sessionId: string,
  container: HTMLElement,
  options: { interactive: boolean; onExit: ExitListener },
): Attachment {
  const entry = entries.get(sessionId) ?? createEntry(sessionId);
  const at = finished.indexOf(sessionId);
  if (at !== -1) finished.splice(at, 1);

  setInteractive(entry, options.interactive);
  entry.container = container;
  if (entry.opened) {
    if (entry.term.element) container.appendChild(entry.term.element);
    entry.term.refresh(0, entry.term.rows - 1);
  } else {
    entry.term.open(container);
    entry.opened = true;
  }

  const onExit: ExitListener = (code) => options.onExit(code);
  entry.exitListeners.add(onExit);
  return {
    get exitCode() {
      return entry.exitCode;
    },
    refit() {
      if (entry.disposed || entry.container !== container) return;
      if (!fitIfShown(entry)) return;
      if (entry.sizeUnsent) {
        entry.sizeUnsent = false;
        sendSize(entry);
      }
    },
    detach() {
      if (entry.disposed) return;
      entry.exitListeners.delete(onExit);
      if (entry.container !== container) return;
      entry.container = null;
      entry.term.element?.remove();
      if (entry.exitCode !== null) settle(entry);
    },
  };
}

/** Changes whether a session's terminal forwards keystrokes. Its output and scrollback are kept. */
export function setTerminalInteractive(sessionId: string, interactive: boolean): void {
  const entry = entries.get(sessionId);
  if (entry) setInteractive(entry, interactive);
}

/** Ends a session's terminal: its subscriptions are released and its output is no longer kept. */
export function disposeTerminal(sessionId: string): void {
  const entry = entries.get(sessionId);
  if (entry) disposeEntry(entry);
}

function createEntry(sessionId: string): Entry {
  const term = new Terminal({
    cursorBlink: false,
    disableStdin: true,
    convertEol: false,
    fontSize: 12,
    fontFamily: 'ui-monospace, Menlo, Monaco, "Cascadia Mono", monospace',
    scrollback: 5000,
    theme: { background: "#09090b", foreground: "#e4e4e7", cursor: "#a5b4fc", selectionBackground: "#3f3f46" },
  });
  const fit = new FitAddon();
  term.loadAddon(fit);
  const entry: Entry = {
    sessionId,
    term,
    fit,
    opened: false,
    container: null,
    interactive: false,
    exitCode: null,
    sizeUnsent: false,
    disposed: false,
    unsubscribe: [],
    exitListeners: new Set(),
  };
  entries.set(sessionId, entry);

  const onData = term.onData((data) => {
    if (entry.interactive) void ipc.writeAgentPty(sessionId, data).catch(() => undefined);
  });
  const onResize = term.onResize(() => sendSize(entry));
  entry.unsubscribe.push(() => onData.dispose(), () => onResize.dispose());

  start(entry).catch((error: unknown) => {
    term.write(`\r\n\x1b[31m[terminal error: ${String(error)}]\x1b[0m\r\n`);
  });
  return entry;
}

/** Subscribes to a session's output, then replays what it printed before the terminal existed. */
async function start(entry: Entry): Promise<void> {
  const { sessionId, term } = entry;
  // Output that arrives before the replay is written is held back, and written after it, in order.
  let live = false;
  const held: string[] = [];
  const write = (data: string) => {
    if (live) term.write(data);
    else held.push(data);
  };

  // Subscribe before replaying history, so nothing emitted during replay is lost.
  // A chunk emitted in that narrow window can appear twice; TUIs redraw past it.
  keep(entry, await events.ptyOutput(sessionId, write));
  keep(
    entry,
    await events.ptyExit(sessionId, (code) => {
      if (entry.disposed) return;
      entry.exitCode = code;
      write(`\r\n\x1b[90m[process exited with code ${code}]\x1b[0m\r\n`);
      for (const listener of entry.exitListeners) listener(code);
      if (entry.container === null) settle(entry);
    }),
  );

  const history = await ipc.getPtyHistory(sessionId).catch(() => "");
  if (entry.disposed) return;
  // Fit before the replay, so it is parsed at the size the terminal is shown at.
  const shown = fitIfShown(entry);
  term.write(history);
  for (const chunk of held) term.write(chunk);
  held.length = 0;
  live = true;
  if (shown) sendSize(entry);
  else entry.sizeUnsent = true;
}

function keep(entry: Entry, unsubscribe: () => void): void {
  if (entry.disposed) unsubscribe();
  else entry.unsubscribe.push(unsubscribe);
}

function setInteractive(entry: Entry, interactive: boolean): void {
  entry.interactive = interactive;
  entry.term.options.cursorBlink = interactive;
  entry.term.options.disableStdin = !interactive;
}

/** Fits the terminal to its container. A hidden container has no size to fit to, so it is left alone. */
function fitIfShown(entry: Entry): boolean {
  const container = entry.container;
  if (!container || container.getClientRects().length === 0) return false;
  try {
    entry.fit.fit();
  } catch {
    // The container can be momentarily zero-sized while panels reflow.
    return false;
  }
  return true;
}

function sendSize(entry: Entry): void {
  void ipc.resizeAgentPty(entry.sessionId, entry.term.cols, entry.term.rows).catch(() => undefined);
}

/**
 * An exited session that no view shows keeps its terminal, so it shows its last output when it is opened
 * again. Past the retention limit the oldest are released, as the backend releases old sessions' output.
 */
function settle(entry: Entry): void {
  const at = finished.indexOf(entry.sessionId);
  if (at !== -1) finished.splice(at, 1);
  finished.push(entry.sessionId);
  while (finished.length > FINISHED_RETENTION) {
    const oldestId = finished.shift();
    const oldest = oldestId === undefined ? undefined : entries.get(oldestId);
    if (oldest) disposeEntry(oldest);
  }
}

function disposeEntry(entry: Entry): void {
  if (entry.disposed) return;
  entry.disposed = true;
  if (entries.get(entry.sessionId) === entry) entries.delete(entry.sessionId);
  const at = finished.indexOf(entry.sessionId);
  if (at !== -1) finished.splice(at, 1);
  for (const unsubscribe of entry.unsubscribe) unsubscribe();
  entry.unsubscribe = [];
  entry.exitListeners.clear();
  entry.term.element?.remove();
  entry.term.dispose();
}
