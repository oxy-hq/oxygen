import { cn } from "@/libs/shadcn/utils";
import type { FleetRow } from "@/services/api/adminObservability";
import { money } from "../format";

/**
 * What a cost to serve is made of, as one bar.
 *
 * **One hue in four lightness steps, never four hues.** On this console colour
 * means something is wrong (`pages/admin/CLAUDE.md`), so a neutral split may
 * not spend it — and the design system's categorical pair fails the dark-mode
 * contrast floor anyway (`2026-11-16-admin-observability-design.md` §8.3).
 * Order and length carry the split; the 1px gaps keep adjacent steps apart.
 */
export interface CostPart {
  key: "llm" | "storage" | "egress" | "compute";
  label: string;
  /** The segment's fill, as a step of the foreground. */
  fill: string;
  /** The same step for an SVG mark. */
  svgFill: string;
  value: (r: CostParts) => number | null;
}

/** The fields a bar reads — a fleet row has them all. */
export type CostParts = Pick<FleetRow, "cost_usd" | "storage_usd" | "egress_usd" | "compute_usd">;

export const COST_PARTS: CostPart[] = [
  {
    key: "llm",
    label: "LLM",
    fill: "bg-foreground",
    svgFill: "fill-foreground",
    value: (r) => r.cost_usd
  },
  {
    key: "storage",
    label: "Storage",
    fill: "bg-foreground/55",
    svgFill: "fill-foreground/55",
    value: (r) => r.storage_usd
  },
  {
    key: "egress",
    label: "Egress",
    fill: "bg-foreground/30",
    svgFill: "fill-foreground/30",
    value: (r) => r.egress_usd
  },
  {
    key: "compute",
    label: "Compute",
    fill: "bg-foreground/15",
    svgFill: "fill-foreground/15",
    value: (r) => r.compute_usd
  }
];

/**
 * A bar whose length is `total / scaleMax` of its container, so every bar on
 * a page shares one scale and the column of bars reads as a chart.
 */
export const CompositionBar = ({
  parts,
  scaleMax,
  className
}: {
  parts: CostParts;
  scaleMax: number;
  className?: string;
}) => {
  const segments = COST_PARTS.map((p) => ({ part: p, value: p.value(parts) ?? 0 })).filter(
    (s) => s.value > 0
  );
  const max = scaleMax > 0 ? scaleMax : 1;
  return (
    <div className={cn("flex h-2 w-full items-stretch gap-px", className)} aria-hidden>
      {segments.map(({ part, value }) => (
        <div
          key={part.key}
          title={`${part.label} ${money(value)}`}
          className={cn("min-w-px rounded-xs", part.fill)}
          style={{ width: `${(value / max) * 100}%` }}
        />
      ))}
    </div>
  );
};

/**
 * The key for every composition bar on the page. Compute is named for what
 * it is — apportioned from a supplied bill — or as unpriced, once, rather than
 * as a segment that is silently always empty.
 */
export const CompositionKey = ({
  computePriced,
  className
}: {
  computePriced: boolean;
  className?: string;
}) => (
  <ul
    className={cn(
      "flex flex-wrap items-center gap-x-3 gap-y-1 text-muted-foreground text-xs",
      className
    )}
    data-testid='admin-observability-composition-key'
  >
    {COST_PARTS.filter((p) => p.key !== "compute" || computePriced).map((p) => (
      <li key={p.key} className='flex items-center gap-1.5'>
        <span className={cn("inline-block h-2 w-2.5 rounded-xs", p.fill)} aria-hidden />
        {p.key === "compute" ? "Compute (apportioned)" : p.label}
      </li>
    ))}
    {computePriced ? null : (
      <li title='Set OXY_FLEET_COMPUTE_USD_MONTH to price function compute as a share of the serving fleet.'>
        Compute unpriced
      </li>
    )}
  </ul>
);
