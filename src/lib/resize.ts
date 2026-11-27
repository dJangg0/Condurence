/** Which way a handle moves: "y" is a horizontal bar that moves up and down, "x" a vertical bar that moves sideways. */
export type ResizeAxis = "x" | "y";

/**
 * Which way the handle has to move to grow the pane it resizes. "start" is up for a bottom pane and
 * left for a right-hand pane. "end" is down for a top pane and right for a left-hand pane.
 */
export type GrowsToward = "start" | "end";

export type SizeBounds = { min: number; max: number };

/** Keeps a size inside its bounds. If the bounds are inverted, such as in a very small window, the minimum wins. */
export function clampSize(size: number, bounds: SizeBounds): number {
  return Math.min(Math.max(size, bounds.min), Math.max(bounds.min, bounds.max));
}

/** Size of a pane after a pointer drag from `startPointer` to `pointer`. */
export function sizeFromDrag(
  startSize: number,
  startPointer: number,
  pointer: number,
  bounds: SizeBounds,
  growsToward: GrowsToward = "start",
): number {
  const distance = pointer - startPointer;
  return clampSize(growsToward === "start" ? startSize - distance : startSize + distance, bounds);
}

const KEYS: Record<ResizeAxis, Record<GrowsToward, string>> = {
  y: { start: "ArrowUp", end: "ArrowDown" },
  x: { start: "ArrowLeft", end: "ArrowRight" },
};

/** Size after an arrow key on a focused handle, or null when the key does not resize on this axis. */
export function sizeFromKey(
  size: number,
  key: string,
  axis: ResizeAxis,
  step: number,
  bounds: SizeBounds,
  growsToward: GrowsToward = "start",
): number | null {
  const growKey = KEYS[axis][growsToward];
  const shrinkKey = KEYS[axis][growsToward === "start" ? "end" : "start"];
  if (key === growKey) return clampSize(size + step, bounds);
  if (key === shrinkKey) return clampSize(size - step, bounds);
  return null;
}
