import type { ComponentType, ReactNode } from "react";
import { card } from "@/components/ui/reference/tokens";
import { cn } from "@/libs/shadcn/utils";

/**
 * Compact horizontal KPI strip pinned under the page header on every Admin
 * detail surface. Denser than `AdminStatCard` (used by overview hubs) and
 * intentionally NOT clickable — the strip is "vital signs at a glance,"
 * not navigation.
 *
 * Cells share an equal-width grid so the row reads as a single rhythm
 * across screen widths. Each cell carries a tiny uppercase label, the value
 * at the hero size `AdminKpiStrip` uses, and an optional sub-line. A value
 * that is zero or empty fades, so the cells with something in them lead.
 */
const isQuietValue = (value: ReactNode): boolean =>
  value === 0 ||
  value === null ||
  value === undefined ||
  value === "" ||
  value === "—" ||
  (typeof value === "string" && /^\$?0(\.0+)?$/.test(value.trim()));

export const AdminDetailStats = ({
  items,
  className
}: {
  items: Array<{
    label: string;
    value: ReactNode;
    /** Optional second line (e.g. "30 days ago" under "Last login"). */
    sub?: ReactNode;
    icon?: ComponentType<{ className?: string }>;
    /** Fade the value even when it is not a literal zero (a "0d"-style reading). */
    quiet?: boolean;
  }>;
  className?: string;
}) => (
  <div
    className={cn(
      card,
      "grid divide-x divide-border-subtle overflow-hidden",
      items.length <= 3 ? "grid-cols-1 sm:grid-cols-3" : "grid-cols-2 sm:grid-cols-4",
      className
    )}
  >
    {items.map((item, idx) => (
      <div key={`${item.label}-${idx}`} className='flex flex-col justify-center gap-0.5 px-4 py-3'>
        <div className='flex items-center gap-1.5 font-medium text-[10px] text-muted-foreground uppercase tracking-[0.14em]'>
          {item.icon ? <item.icon className='size-3' /> : null}
          {item.label}
        </div>
        <div
          className={cn(
            "font-semibold text-2xl tabular-nums tracking-tight",
            (item.quiet ?? isQuietValue(item.value)) && "text-muted-foreground/60"
          )}
        >
          {item.value}
        </div>
        {item.sub ? (
          <div className='truncate text-muted-foreground text-xs tabular-nums'>{item.sub}</div>
        ) : null}
      </div>
    ))}
  </div>
);
