import type { ReactNode } from "react";
import { Link } from "react-router-dom";
import { cn } from "@/libs/shadcn/utils";
import { AdminInfo } from "./AdminInfo";
import { ADMIN_TONE, type AdminTone } from "./adminTone";

export type AdminKpi = {
  /** Stable id: drives the testid, so renaming the label cannot break a test. */
  id: string;
  /** One or two words. The unit or window goes in `caption`, not here. */
  label: string;
  /**
   * The number, already formatted. `null` is "not measured", drawn as a muted dash,
   * never as 0.
   */
  value: ReactNode | null;
  /** One short line under the number: the window, the unit, the comparison. */
  caption?: ReactNode;
  /**
   * Colour only when something is wrong (`warn`, `danger`). A healthy number stays in
   * the foreground colour, like every other number on this surface.
   */
  tone?: Extract<AdminTone, "warn" | "danger">;
  /** True when the value is zero or empty: the number fades so the others lead. */
  quiet?: boolean;
  /** How this is measured, behind an ⓘ beside the label. */
  info?: ReactNode;
  /** Makes the cell a link to where the number comes from. */
  to?: string;
};

/**
 * A row of numbers that answer a page before anything else on it does.
 *
 * Each cell is a small uppercase label, one number at the hero size, and at most one
 * line of caption. Cells share one bordered strip with hairline dividers, rather than
 * being cards of their own: they are one reading of one thing, and a row of identical
 * cards would say they are separate objects.
 *
 * The strip is for the handful of numbers a page leads with (four to six). A page with
 * twelve numbers has a table, not a strip.
 */
export const AdminKpiStrip = ({
  items,
  className,
  "data-testid": dataTestId
}: {
  items: AdminKpi[];
  className?: string;
  "data-testid"?: string;
}) => (
  // Every cell draws its own right and bottom hairline; the grid sits one pixel past
  // the frame on both edges so the last column's and last row's lines fall under the
  // frame's clip. That is what keeps a strip of five on a three-column phone layout
  // from showing a stray line under a half-empty row.
  <div
    data-testid={dataTestId}
    className={cn("overflow-hidden rounded-lg border border-border/60 bg-card", className)}
  >
    <div className={cn("-mr-px -mb-px grid grid-cols-2 sm:grid-cols-3", LG_COLUMNS[items.length])}>
      {items.map((item) => (
        <KpiCell key={item.id} item={item} testId={dataTestId} />
      ))}
    </div>
  </div>
);

/** Static class names, so Tailwind sees each one. Six is the most a strip holds. */
const LG_COLUMNS: Record<number, string> = {
  1: "lg:grid-cols-1",
  2: "lg:grid-cols-2",
  3: "lg:grid-cols-3",
  4: "lg:grid-cols-4",
  5: "lg:grid-cols-5",
  6: "lg:grid-cols-6"
};

const KpiCell = ({ item, testId }: { item: AdminKpi; testId?: string }) => {
  const { id, label, value, caption, tone, quiet, info, to } = item;
  const body = (
    <>
      <div className='flex items-center gap-1 font-medium text-[10px] text-muted-foreground uppercase tracking-[0.14em]'>
        <span className='truncate'>{label}</span>
        {info ? <AdminInfo>{info}</AdminInfo> : null}
      </div>
      <div
        data-testid={testId ? `${testId}-${id}-value` : undefined}
        className={cn(
          "mt-1 truncate font-semibold text-2xl tabular-nums tracking-tight",
          value === null || quiet
            ? "text-muted-foreground/60"
            : tone
              ? ADMIN_TONE[tone].text
              : "text-foreground"
        )}
      >
        {value === null ? "—" : value}
      </div>
      {caption ? (
        <div className='mt-0.5 truncate text-muted-foreground text-xs'>{caption}</div>
      ) : null}
    </>
  );
  const cell = cn(
    "min-w-0 border-border/60 border-r border-b px-4 py-3",
    to && "transition-colors hover:bg-muted/40 focus-visible:bg-muted/40 focus-visible:outline-none"
  );
  const cellTestId = testId ? `${testId}-${id}` : undefined;
  return to ? (
    <Link to={to} className={cell} data-testid={cellTestId}>
      {body}
    </Link>
  ) : (
    <div className={cell} data-testid={cellTestId}>
      {body}
    </div>
  );
};
