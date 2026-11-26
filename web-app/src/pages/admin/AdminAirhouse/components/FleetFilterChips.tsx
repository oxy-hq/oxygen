import { cn } from "@/libs/shadcn/utils";
import { AdminStatusPill, type AdminStatusTone } from "@/pages/admin/components/AdminStatusPill";
import { SEVERITY_LABEL, SEVERITY_ORDER, type Severity } from "../severity";

/**
 * The fleet summary, and the filter, as one control.
 *
 * A count an operator can read but not act on is a dead end: "2 without a
 * service account" tells them something is wrong and then makes them find it by
 * eye down a list of every workspace. Making the count the filter collapses
 * report and action into one click, which is the whole interaction budget an
 * incident gets.
 *
 * Severities with no rows render disabled rather than hidden. A missing chip
 * would read as "no such state exists"; a greyed one reads as "none right now",
 * and the row of chips keeps a stable shape as the fleet changes underneath it.
 */
const TONE: Record<Severity, AdminStatusTone> = {
  broken: "danger",
  degraded: "warn",
  healthy: "ok"
};

export const FleetFilterChips = ({
  counts,
  total,
  active,
  onChange
}: {
  counts: Record<Severity, number>;
  total: number;
  active: Severity | null;
  onChange: (next: Severity | null) => void;
}) => (
  <div className='flex flex-wrap items-center gap-1' data-testid='admin-airhouse-filters'>
    <button
      type='button'
      onClick={() => onChange(null)}
      className={cn(
        " border px-1.5 py-0.5 text-xs transition-colors",
        active === null
          ? "border-border bg-muted text-foreground"
          : "border-transparent text-muted-foreground hover:bg-muted/50"
      )}
      data-testid='admin-airhouse-filter-all'
      aria-pressed={active === null}
    >
      All <span className='tabular-nums'>{total}</span>
    </button>

    {SEVERITY_ORDER.map((severity) => {
      const count = counts[severity];
      const selected = active === severity;
      return (
        <button
          key={severity}
          type='button'
          disabled={count === 0}
          onClick={() => onChange(selected ? null : severity)}
          className={cn(
            " border px-1.5 py-0.5 transition-colors",
            selected
              ? "border-border bg-muted"
              : "border-transparent hover:bg-muted/50 disabled:hover:bg-transparent",
            count === 0 && "opacity-40"
          )}
          data-testid={`admin-airhouse-filter-${severity}`}
          aria-pressed={selected}
        >
          <AdminStatusPill tone={TONE[severity]} label={`${SEVERITY_LABEL[severity]} ${count}`} />
        </button>
      );
    })}
  </div>
);
