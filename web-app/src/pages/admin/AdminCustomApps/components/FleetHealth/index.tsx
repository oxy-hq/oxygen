import { useMemo, useState } from "react";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import { useFleetHealth } from "@/hooks/api/customApps/useFleetHealth";
import type { AppHealth } from "@/types/apps";
import { relativeTime } from "../AppDetail/components/Activity/relativeTime";
import { HealthSummary } from "./HealthSummary";
import { HealthTable } from "./HealthTable";

/**
 * Fleet health — every published custom app and whether it is working.
 *
 * See `internal-docs/2026-09-16-custom-app-fleet-observability-design.md`.
 *
 * The table's job is to be honest about what is *not* known. Two of its five
 * verdicts are non-answers — an app below the traffic floor is `Quiet`, an app
 * nobody measured is `Not measured` — and neither is ever rendered as healthy.
 * That is the whole reason this page exists: the signal was already being
 * collected, and every path to it collapsed an absent measurement into a green
 * tick.
 */
export const FleetHealth = () => {
  const [filter, setFilter] = useState<AppHealth | null>(null);
  const { data, isLoading, isError } = useFleetHealth();

  const visible = useMemo(
    () => (filter ? (data?.apps ?? []).filter((a) => a.health === filter) : (data?.apps ?? [])),
    [data?.apps, filter]
  );

  if (isLoading) {
    return (
      <div className='space-y-3 p-4'>
        <Skeleton className='h-12 w-full' />
        <Skeleton className='h-64 w-full' />
      </div>
    );
  }

  if (isError || !data) {
    return (
      <div className='p-4' data-testid='admin-fleet-health-error'>
        <div className='rounded-md border border-destructive/40 bg-destructive/5 p-4'>
          <p className='font-medium text-sm'>Fleet health did not load.</p>
          <p className='mt-1 text-muted-foreground text-xs'>
            The apps may be fine — this is the page failing, not a verdict about them. Retry in a
            moment; if it persists, check that the observability store is reachable.
          </p>
        </div>
      </div>
    );
  }

  return (
    <div className='space-y-3 overflow-y-auto p-4' data-testid='admin-fleet-health'>
      {/* Capture off is a platform gap, not a per-app problem: every row would
          be `not_measured` for the same reason, so it is explained once here
          rather than repeated down a column. */}
      {!data.observability_configured && (
        <div
          className='rounded-md border border-amber-500/40 bg-amber-500/5 p-3'
          data-testid='admin-fleet-health-capture-off'
        >
          <p className='font-medium text-sm'>No app is being watched.</p>
          <p className='mt-1 text-muted-foreground text-xs'>
            Observability capture is off, so every app below reads as not measured. Set{" "}
            <code className='rounded bg-muted px-1 py-0.5'>OXY_OBSERVABILITY_BACKEND</code> to start
            measuring.
          </p>
        </div>
      )}

      <HealthSummary summary={data.summary} active={filter} onSelect={setFilter} />

      <HealthTable apps={visible} />

      {/* Truncation says so. A table that silently stops at the page cap reads
          as "that is the whole fleet", which on this surface is the same class
          of lie as reporting an unmeasured app as healthy. */}
      {data.has_more && (
        <p
          className='rounded-md border border-dashed px-3 py-2 text-muted-foreground text-xs'
          data-testid='admin-fleet-health-truncated'
        >
          Showing {data.apps.length} of {data.total} published apps. The rest are not on this page —
          the counts above cover this page only.
        </p>
      )}

      {/* The age of the answer, not just the answer. Without it a dead
          evaluator and a quiet fleet look identical. */}
      <p className='text-muted-foreground text-xs' data-testid='admin-fleet-health-evaluated'>
        Checked {relativeTime(data.evaluated_at)}
      </p>
    </div>
  );
};

export default FleetHealth;
