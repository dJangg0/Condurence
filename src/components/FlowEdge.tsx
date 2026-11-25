import { createContext, useContext } from "react";
import { BaseEdge, getBezierPath, type EdgeProps } from "@xyflow/react";
import type { EdgeFlowState } from "../lib/flow";

/** Which edges are carrying a message right now, provided by the canvas during a run. */
export const EdgeFlowContext = createContext<EdgeFlowState>({});

const OUT_COLOR = "#818cf8";
const BACK_COLOR = "#34d399";
const DOT_COUNT = 3;
const DOT_PERIOD_S = 1.2;

function prefersReducedMotion(): boolean {
  return typeof window !== "undefined" && window.matchMedia?.("(prefers-reduced-motion: reduce)").matches === true;
}

/**
 * The routing edge. Idle edges render as plain lines. While a message is in flight, dots travel
 * along the line from the caller to the target; when the result returns they travel back, in
 * green. Reduced-motion users get a static highlight instead of moving dots.
 */
export function FlowEdge(props: EdgeProps) {
  const flow = useContext(EdgeFlowContext)[props.id];
  const [path, labelX, labelY] = getBezierPath(props);
  const color = flow === "back" ? BACK_COLOR : OUT_COLOR;
  const moving = flow !== undefined && !prefersReducedMotion();

  return (
    <>
      <BaseEdge
        id={props.id}
        path={path}
        labelX={labelX}
        labelY={labelY}
        label={props.label}
        labelStyle={props.labelStyle}
        labelShowBg={props.labelShowBg}
        labelBgStyle={props.labelBgStyle}
        labelBgPadding={props.labelBgPadding}
        labelBgBorderRadius={props.labelBgBorderRadius}
        markerEnd={props.markerEnd}
        style={flow ? { ...props.style, stroke: color, strokeWidth: 2.5 } : props.style}
      />
      {flow ? (
        <path
          d={path}
          fill="none"
          stroke={color}
          strokeWidth={4}
          strokeOpacity={0.25}
          className={moving ? (flow === "out" ? "oc-flow-out" : "oc-flow-back") : undefined}
          strokeDasharray={moving ? "8 8" : undefined}
          pointerEvents="none"
          aria-hidden
        />
      ) : null}
      {moving
        ? Array.from({ length: DOT_COUNT }, (_, index) => (
            <circle key={index} r={4.5} fill={color} pointerEvents="none" aria-hidden>
              <animateMotion
                dur={`${DOT_PERIOD_S}s`}
                begin={`${(index * DOT_PERIOD_S) / DOT_COUNT}s`}
                repeatCount="indefinite"
                path={path}
                // `back` runs the same path from its end to its start.
                keyPoints={flow === "back" ? "1;0" : "0;1"}
                keyTimes="0;1"
                calcMode="linear"
              />
            </circle>
          ))
        : null}
    </>
  );
}
