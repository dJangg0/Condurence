import { useEffect, useRef, useState } from "react";
import "@xterm/xterm/css/xterm.css";
import { attachTerminal, setTerminalInteractive } from "../lib/terminalStore";

type Props = {
  sessionId: string;
  /** Read-only inspection (delegated runs) when false; keystrokes are forwarded when true. */
  interactive?: boolean;
};

/**
 * Shows a PTY session's xterm.js terminal. The terminal itself lives in the terminal store, so unmounting
 * this pane keeps its output, scrollback and size. Mounting it again shows the same terminal, with nothing
 * replayed.
 */
export function TerminalPane({ sessionId, interactive = false }: Props) {
  const containerRef = useRef<HTMLDivElement>(null);
  // Read when the pane attaches, so a change does not reattach the live terminal.
  const interactiveRef = useRef(interactive);
  const [exitCode, setExitCode] = useState<number | null>(null);

  // A run starting or ending flips `interactive`. It is applied to the live terminal, which keeps its output.
  useEffect(() => {
    interactiveRef.current = interactive;
    setTerminalInteractive(sessionId, interactive);
  }, [sessionId, interactive]);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    const attachment = attachTerminal(sessionId, container, {
      interactive: interactiveRef.current,
      onExit: setExitCode,
    });
    setExitCode(attachment.exitCode);
    // A pane that mounts hidden is fitted, and its process sized, once it is shown. Later changes come here too.
    const observer = new ResizeObserver(() => attachment.refit());
    observer.observe(container);
    return () => {
      observer.disconnect();
      attachment.detach();
    };
  }, [sessionId]);

  return (
    <div className="relative h-full w-full bg-zinc-950 p-2">
      <div ref={containerRef} className="h-full w-full" />
      {exitCode !== null ? (
        <span className="pointer-events-none absolute right-3 top-3 rounded bg-zinc-800 px-2 py-0.5 text-[10px] text-zinc-400">
          exited {exitCode}
        </span>
      ) : null}
    </div>
  );
}
