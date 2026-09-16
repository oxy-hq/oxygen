import { cn } from "@/libs/shadcn/utils";
import type { AppHealth, FleetSummary } from "@/types/apps";
import { HEALTH_LABEL, HealthDot } from "./HealthDot";

/**
 * Counts by verdict, worst first — the same order the table sorts in, so the
 * strip reads as a key to the rows beneath it rather than a separate scoreboard.
 */
const ORDER: AppHealth[] = ["down", "degraded", "not_measured", "quiet", "operational"];

export const HealthSummary = ({
  summary,
  active,
  onSelect
}: {
  summary: FleetSummary;
  /** The verdict currently filtered to, if any. */
  active: AppHealth | null;
  onSelect: (health: AppHealth | null) => void;
}) => (
  <div className='flex flex-wrap gap-2' data-testid='admin-fleet-health-summary'>
    {ORDER.map((health) => {
      const count = summary[health];
      const selected = active === health;
      return (
        <button
          key={health}
          type='button'
          // Clicking a count filters the table to it. A count nobody can act on
          // is a number; a count that narrows the list is a control.
          onClick={() => onSelect(selected ? null : health)}
          aria-pressed={selected}
          className={cn(
            "flex items-center gap-2 rounded-md border bg-card px-3 py-2 text-left transition-colors",
            "hover:bg-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
            selected && "border-foreground/30 bg-accent",
            // A zero is shown, not hidden: "no apps are down" is information,
            // and a strip whose columns move between refreshes is unreadable.
            count === 0 && !selected && "opacity-60"
          )}
          data-testid={`admin-fleet-health-count-${health}`}
        >
          <HealthDot health={health} decorative />
          <span className='font-medium text-sm tabular-nums'>{count}</span>
          <span className='text-muted-foreground text-xs'>{HEALTH_LABEL[health]}</span>
        </button>
      );
    })}
  </div>
);
