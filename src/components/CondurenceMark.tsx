/**
 * The Condurence "Lanes" mark: three concentric tracks that start on one line and finish at
 * different angles, each ending in a node (parallel work that lands when it lands).
 * Geometry matches `assets/condurence-app-icon.svg`, drawn in the reference's 960-unit tile space.
 */

const CENTER = 480;
const START_DEG = 41;
const STROKE = 44;
const NODE_R = 44;
const TRACK_COLOR = "#6d6bed";
const NODE_COLOR = "#b5bcf8";
/** Animated tracks draw at one angular speed, so longer tracks take longer and land their node last. */
const DRAW_MS_PER_DEGREE = 3.25;

/** Outer to inner track: centreline radius and the angle where the track finishes. */
const TRACKS = [
  { r: 231, end: 318 },
  { r: 154, end: 288 },
  { r: 77, end: 252 },
];

function pointAt(r: number, deg: number): [number, number] {
  const a = (deg * Math.PI) / 180;
  return [CENTER + r * Math.cos(a), CENTER + r * Math.sin(a)];
}

function arcPath(r: number, end: number): string {
  const [x0, y0] = pointAt(r, START_DEG);
  const [x1, y1] = pointAt(r, end);
  const largeArc = end - START_DEG > 180 ? 1 : 0;
  return `M${x0} ${y0} A${r} ${r} 0 ${largeArc} 1 ${x1} ${y1}`;
}

/**
 * `animate` draws the tracks in from their shared start and lands each node when its track finishes.
 * The motion lives in `oc-splash-*` classes (index.css), which only apply when reduced motion is not requested.
 */
export function CondurenceMark({ className, animate = false }: { className?: string; animate?: boolean }) {
  return (
    <svg viewBox="180 200 560 560" className={className} aria-hidden="true">
      {TRACKS.map(({ r, end }) => {
        const drawMs = (end - START_DEG) * DRAW_MS_PER_DEGREE;
        return (
          <path
            key={`track-${r}`}
            d={arcPath(r, end)}
            fill="none"
            stroke={TRACK_COLOR}
            strokeWidth={STROKE}
            strokeLinecap="round"
            pathLength={animate ? 1 : undefined}
            className={animate ? "oc-splash-track" : undefined}
            style={animate ? { animationDuration: `${drawMs}ms` } : undefined}
          />
        );
      })}
      {TRACKS.map(({ r, end }) => {
        const [cx, cy] = pointAt(r, end);
        const landMs = (end - START_DEG) * DRAW_MS_PER_DEGREE;
        return (
          <circle
            key={`node-${r}`}
            cx={cx}
            cy={cy}
            r={NODE_R}
            fill={NODE_COLOR}
            className={animate ? "oc-splash-node" : undefined}
            style={animate ? { animationDelay: `${landMs}ms` } : undefined}
          />
        );
      })}
    </svg>
  );
}
