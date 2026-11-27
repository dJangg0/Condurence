import { events, ipc } from "./ipc";

type Entry = {
  listeners: Set<() => void>;
  stop?: () => void;
  /** The listener was released before the backend answered, so it is dropped on arrival. */
  released: boolean;
  teardown?: ReturnType<typeof setTimeout>;
};

const entries = new Map<string, Entry>();

/** How long a watcher outlives its last listener, so a quick unmount and remount keeps it. */
const GRACE_MS = 250;

/**
 * Calls `onDirty` whenever files in the workspace change, once the backend has seen a burst of edits
 * settle. The backend keeps one watcher per workspace and cannot count its users, so the watcher is
 * shared here: it starts with the first listener and stops after the last one leaves. Without that,
 * a panel closing would silently stop the toolbar's live count watching the same workspace.
 * Returns the function that stops listening.
 */
export function watchWorkspaceDirty(path: string, onDirty: () => void): () => void {
  let entry = entries.get(path);
  if (entry) {
    if (entry.teardown !== undefined) clearTimeout(entry.teardown);
    entry.teardown = undefined;
  } else {
    const created: Entry = { listeners: new Set(), released: false };
    entries.set(path, created);
    entry = created;
    void ipc
      .watchWorkspace(path)
      .then((workspaceId) =>
        events.workspaceDirty(workspaceId, () => created.listeners.forEach((listener) => listener())),
      )
      .then((stop) => {
        if (created.released) stop();
        else created.stop = stop;
      })
      .catch(() => undefined);
  }
  const mine = entry;
  mine.listeners.add(onDirty);

  return () => {
    mine.listeners.delete(onDirty);
    if (mine.listeners.size > 0 || mine.teardown !== undefined) return;
    mine.teardown = setTimeout(() => {
      if (mine.listeners.size > 0 || entries.get(path) !== mine) return;
      entries.delete(path);
      mine.released = true;
      mine.stop?.();
      void ipc.unwatchWorkspace(path).catch(() => undefined);
    }, GRACE_MS);
  };
}
