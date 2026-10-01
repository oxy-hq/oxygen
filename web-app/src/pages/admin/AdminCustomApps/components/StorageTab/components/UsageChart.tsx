import { useMemo } from "react";
import EChart from "@/components/Echarts/EChart";
import { useStorageHistory } from "@/hooks/api/customApps/useAppStorage";
import { cn } from "@/libs/shadcn/utils";
import useTheme from "@/stores/useTheme";
import { usageOverTimeOption } from "../chartSpec";

const RANGES = [7, 30, 90] as const;
export type Range = (typeof RANGES)[number];

interface Props {
  days: Range;
  onDaysChange: (days: Range) => void;
  /** Omit for the fleet total; set to chart one app. */
  appId?: string;
  title: string;
  height?: number;
  /** Suffix for the testid — this component mounts twice on the page. */
  id: string;
}

/**
 * Storage held over time.
 *
 * The signature chart of this surface: a level going up and to the right is the
 * thing an operator is here to catch, and it is invisible in a table of current
 * sizes. One series, so no legend — the heading names it.
 *
 * Points are values *held* at each day's end, not that day's writes, so a flat
 * line means "nothing changed" rather than "nothing was measured".
 */
export function UsageChart({ days, onDaysChange, appId, title, height = 160, id }: Props) {
  const { data, isLoading, error } = useStorageHistory(days, appId);
  const points = useMemo(() => data?.points ?? [], [data]);
  // `resolveColor` reads getComputedStyle when the option is built, and EChart
  // only re-renders on option identity — so without the theme in the deps a
  // light→dark toggle leaves the line, grid and segment gaps in the old palette
  // until something else invalidates the query.
  const theme = useTheme((s) => s.theme);
  const option = useMemo(() => usageOverTimeOption(points, theme), [points, theme]);

  // A failed fetch also yields an empty `points`, so branch on the error FIRST —
  // otherwise a 500 renders as the reassuring "no samples yet" empty state.
  const empty = !isLoading && !error && points.every((p) => p.bytes === 0);

  return (
    <section
      className='rounded-md border bg-card p-2'
      data-testid={`admin-storage-usage-chart-${id}`}
    >
      <header className='mb-1 flex items-center justify-between'>
        <h3 className='font-semibold text-sm'>{title}</h3>
        {/* fieldset/legend rather than role="group": the semantic element
            carries the grouping for assistive tech without an ARIA override. */}
        <fieldset className='flex gap-1 border-0 p-0'>
          <legend className='sr-only'>Time range</legend>
          {RANGES.map((r) => (
            <button
              key={r}
              type='button'
              onClick={() => onDaysChange(r)}
              aria-pressed={days === r}
              className={cn(
                "rounded px-2 py-0.5 text-xs",
                days === r
                  ? "bg-muted font-medium text-foreground"
                  : "text-muted-foreground hover:text-foreground"
              )}
            >
              {r}d
            </button>
          ))}
        </fieldset>
      </header>
      {error ? (
        <p className='py-6 text-center text-destructive text-xs'>Could not load usage history.</p>
      ) : empty ? (
        // A flat line at zero is indistinguishable from a broken chart, so say
        // which one it is.
        <p className='py-6 text-center text-muted-foreground text-xs'>
          No samples in this window. The sweeper records one per app per run.
        </p>
      ) : (
        <EChart option={option} height={height} loading={isLoading} />
      )}
    </section>
  );
}
