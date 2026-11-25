import { useEffect, useRef, useState } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { events, ipc } from "../lib/ipc";

type Props = {
  sessionId: string;
  /** Read-only inspection (delegated runs) when false; keystrokes are forwarded when true. */
  interactive?: boolean;
};

/**
 * xterm.js attached to a native PTY. On mount it subscribes to live output, replays the
 * backend ring buffer (so remounting never loses context), then forwards input and resizes.
 */
export function TerminalPane({ sessionId, interactive = false }: Props) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [exitCode, setExitCode] = useState<number | null>(null);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    let disposed = false;
    const unlisteners: Array<() => void> = [];
    const track = (unlisten: () => void) => {
      if (disposed) unlisten();
      else unlisteners.push(unlisten);
    };

    const term = new Terminal({
      cursorBlink: interactive,
      disableStdin: !interactive,
      convertEol: false,
      fontSize: 12,
      fontFamily: 'ui-monospace, Menlo, Monaco, "Cascadia Mono", monospace',
      scrollback: 5000,
      theme: { background: "#09090b", foreground: "#e4e4e7", cursor: "#a5b4fc", selectionBackground: "#3f3f46" },
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(container);

    const setup = async () => {
      // Subscribe before replaying history, so nothing emitted during replay is lost.
      // A chunk emitted in that narrow window can appear twice; TUIs redraw past it.
      let ready = false;
      const pending: string[] = [];
      track(
        await events.ptyOutput(sessionId, (chunk) => {
          if (ready) term.write(chunk);
          else pending.push(chunk);
        }),
      );
      track(
        await events.ptyExit(sessionId, (code) => {
          if (disposed) return;
          setExitCode(code);
          term.write(`\r\n\x1b[90m[process exited with code ${code}]\x1b[0m\r\n`);
        }),
      );

      const history = await ipc.getPtyHistory(sessionId).catch(() => "");
      if (disposed) return;
      term.write(history);
      for (const chunk of pending) term.write(chunk);
      pending.length = 0;
      ready = true;
      fit.fit();
      await ipc.resizeAgentPty(sessionId, term.cols, term.rows).catch(() => undefined);
    };

    const onData = term.onData((data) => {
      if (interactive) void ipc.writeAgentPty(sessionId, data).catch(() => undefined);
    });
    const onResize = term.onResize(({ cols, rows }) => {
      void ipc.resizeAgentPty(sessionId, cols, rows).catch(() => undefined);
    });
    const observer = new ResizeObserver(() => {
      try {
        fit.fit();
      } catch {
        // The container can be momentarily zero-sized while panels reflow.
      }
    });
    observer.observe(container);

    setup().catch((error: unknown) => {
      term.write(`\r\n\x1b[31m[terminal error: ${String(error)}]\x1b[0m\r\n`);
    });

    return () => {
      disposed = true;
      observer.disconnect();
      onData.dispose();
      onResize.dispose();
      unlisteners.forEach((unlisten) => unlisten());
      term.dispose();
    };
  }, [sessionId, interactive]);

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
