import { BaseEdge, type EdgeProps, getBezierPath } from "@xyflow/react";
import { useEffect, useState } from "react";

/** How long the travelling dot takes from one end to the other. */
const TRAVEL_SECONDS = 2.4;

/** What a data-flow line carries beyond the stock edge props. */
export type FlowEdgeData = { moving: boolean; stroke: string };

const REDUCED = "(prefers-reduced-motion: reduce)";

/** Whether the viewer asked for less motion; follows a change while the page is open. */
export function useReducedMotion(): boolean {
  const [reduced, setReduced] = useState(
    () => typeof window !== "undefined" && window.matchMedia?.(REDUCED).matches === true
  );
  useEffect(() => {
    const query = window.matchMedia?.(REDUCED);
    if (!query) return;
    const onChange = () => setReduced(query.matches);
    query.addEventListener("change", onChange);
    return () => query.removeEventListener("change", onChange);
  }, []);
  return reduced;
}

/**
 * A bezier line that, when `moving`, sends a dot along itself from source to
 * target: the way the data runs. SVG's own `animateMotion`, so nothing ticks in
 * React; the caller leaves `moving` off for a viewer who asked for reduced motion.
 */
export const FlowEdge = (props: EdgeProps) => {
  const [path, labelX, labelY] = getBezierPath(props);
  const data = props.data as FlowEdgeData | undefined;
  return (
    <>
      <BaseEdge
        id={props.id}
        path={path}
        style={props.style}
        markerEnd={props.markerEnd}
        label={props.label}
        labelX={labelX}
        labelY={labelY}
        labelStyle={props.labelStyle}
      />
      {data?.moving && (
        <circle r={2.5} fill={data.stroke} opacity={0.9} data-testid='admin-data-flow-edge-dot'>
          <animateMotion dur={`${TRAVEL_SECONDS}s`} repeatCount='indefinite' path={path} />
        </circle>
      )}
    </>
  );
};
