import { useEffect, useLayoutEffect, useRef, type KeyboardEvent, type PointerEvent } from "react";
import { sizeFromDrag, sizeFromKey, type GrowsToward, type ResizeAxis, type SizeBounds } from "../lib/resize";

/** Pixels per arrow-key press on a focused handle. */
const KEY_STEP = 24;

type Props = {
  /** "y": a horizontal bar, dragged up and down. "x": a vertical bar, dragged sideways. */
  axis: ResizeAxis;
  label: string;
  size: number;
  onResize: (size: number) => void;
  /** Read on every move, so a window resize is respected while a drag is in progress. */
  bounds: () => SizeBounds;
  /**
   * Which way the handle moves to grow the pane it resizes. Defaults to "start": a bottom pane or a
   * right-hand pane, whose handle sits on its leading edge. Use "end" for a top or left-hand pane.
   */
  growsToward?: GrowsToward;
  className?: string;
};

/** A draggable divider between two panes. Arrow keys resize the same pane when the handle is focused. */
export function PaneHandle({ axis, label, size, onResize, bounds, growsToward = "start", className = "" }: Props) {
  // The latest props. A drag's window listeners outlive the render that started them, so they read from here.
  const live = useRef({ size, onResize, bounds, growsToward });
  useLayoutEffect(() => {
    live.current = { size, onResize, bounds, growsToward };
  });

  // Detaches the listeners of a drag in progress, so unmounting mid-drag leaves nothing behind.
  const endDragRef = useRef<(() => void) | null>(null);
  useEffect(() => () => endDragRef.current?.(), []);

  const coordinate = (event: { clientX: number; clientY: number }) => (axis === "y" ? event.clientY : event.clientX);

  const onPointerDown = (event: PointerEvent<HTMLDivElement>) => {
    if (event.button !== 0) return;
    endDragRef.current?.();
    const startPointer = coordinate(event);
    const startSize = live.current.size;

    // The listeners sit on the window rather than the handle. The handle moves as the pane resizes,
    // so a drag must not depend on the pointer staying over it.
    const onMove = (move: globalThis.PointerEvent) => {
      const current = live.current;
      current.onResize(sizeFromDrag(startSize, startPointer, coordinate(move), current.bounds(), current.growsToward));
    };
    const endDrag = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", endDrag);
      window.removeEventListener("pointercancel", endDrag);
      endDragRef.current = null;
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", endDrag);
    window.addEventListener("pointercancel", endDrag);
    endDragRef.current = endDrag;
  };

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const next = sizeFromKey(size, event.key, axis, KEY_STEP, bounds(), growsToward);
    if (next === null) return;
    event.preventDefault();
    onResize(next);
  };

  const orientation = axis === "y" ? "horizontal" : "vertical";
  const cursor = axis === "y" ? "cursor-row-resize" : "cursor-col-resize";
  const thickness = axis === "y" ? "h-2 w-full border-t" : "w-2 self-stretch border-l";

  return (
    <div
      role="separator"
      aria-orientation={orientation}
      aria-label={label}
      aria-valuenow={Math.round(size)}
      tabIndex={0}
      onPointerDown={onPointerDown}
      onKeyDown={onKeyDown}
      className={`shrink-0 touch-none select-none border-zinc-800 transition-colors hover:bg-indigo-500/40 focus-visible:bg-indigo-500/60 focus-visible:outline-none ${cursor} ${thickness} ${className}`}
    />
  );
}
