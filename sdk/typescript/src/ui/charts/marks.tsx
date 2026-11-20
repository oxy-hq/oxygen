// The point marks the two charts draw, in the shapes an app picks by prop (decision 38b: a mark
// drawn on a chart keeps the app's own shape; the design system's corner rule is for controls and
// surfaces, not data marks). The reference's are circles: the sparkline's hollow end ring, the
// trend chart's solid end and hover dots, each haloed in the chart's ground so it stays legible
// where it crosses the other line.

/** A point mark's shape. `none` draws nothing. */
export type PointShape = "ring" | "dot" | "square" | "none";

export interface PointMarkProps {
  shape: PointShape;
  x: number;
  y: number;
  /** The radius; a square's half-side. */
  r: number;
  /** The series colour, a CSS colour (a `var()` of a theme token). */
  colour: string;
}

/** The chart's own ground, for halos and hollow centres (charts.css: the card by default). */
const GROUND = "var(--oxy-ui-chart-ground)";

export function PointMark({ shape, x, y, r, colour }: PointMarkProps) {
  if (shape === "none") return null;
  const hollow = shape === "ring";
  const paint = {
    fill: hollow ? GROUND : colour,
    stroke: hollow ? colour : GROUND,
    strokeWidth: 2,
    "data-mark": shape
  };
  if (shape === "square")
    return <rect x={x - r} y={y - r} width={r * 2} height={r * 2} {...paint} />;
  return <circle cx={x} cy={y} r={r} {...paint} />;
}
