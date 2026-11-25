import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { TerminalPane } from "./TerminalPane";
import { events, ipc, newSessionId } from "../lib/ipc";

const BUTTON_PRIMARY =
  "rounded-md bg-indigo-600 px-2.5 py-1.5 text-xs font-medium text-white hover:bg-indigo-500 disabled:cursor-not-allowed disabled:opacity-50";

/** Starts `gh auth login` in a PTY and returns the session id to show in `SignInDialog`. */
export async function startGitHubSignIn(): Promise<string> {
  const sessionId = newSessionId();
  await ipc.githubStartSignIn(sessionId);
  return sessionId;
}

/** Starts a provider CLI's own login flow in a PTY and returns the session id. */
export async function startProviderSignIn(provider: string): Promise<string> {
  const sessionId = newSessionId();
  await ipc.providerStartSignIn(sessionId, provider);
  return sessionId;
}

/**
 * Runs a CLI's login flow in a PTY, shown with the normal terminal pane. Closing it before the
 * process exits stops the process, so no half-finished login is left running.
 */
export function SignInDialog({
  title,
  sessionId,
  onDone,
}: {
  title: string;
  sessionId: string;
  onDone: () => void;
}) {
  const [finished, setFinished] = useState(false);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void events.ptyExit(sessionId, () => {
      if (!disposed) setFinished(true);
    }).then((fn) => {
      if (disposed) fn();
      else unlisten = fn;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [sessionId]);

  const close = () => {
    if (!finished) void ipc.killAgentPty(sessionId).catch(() => undefined);
    onDone();
  };

  return createPortal(
    <div
      role="dialog"
      aria-modal="true"
      aria-label={title}
      className="fixed inset-0 z-[60] flex items-center justify-center bg-black/60 p-8"
    >
      <div className="flex h-[min(480px,80vh)] w-[min(720px,100%)] flex-col overflow-hidden rounded-lg border border-zinc-700 bg-zinc-950 shadow-2xl">
        <div className="flex items-center gap-3 border-b border-zinc-800 px-4 py-2.5">
          <div className="min-w-0 flex-1">
            <div className="text-sm font-semibold text-zinc-100">{title}</div>
            <div className="text-[11px] text-zinc-500">
              {finished
                ? "Sign-in finished. Click Done to refresh."
                : "Follow the prompts in the terminal. A browser window may open to finish signing in."}
            </div>
          </div>
          <button type="button" onClick={close} className={BUTTON_PRIMARY}>
            Done
          </button>
        </div>
        <div className="min-h-0 flex-1">
          <TerminalPane sessionId={sessionId} interactive />
        </div>
      </div>
    </div>,
    document.body,
  );
}
