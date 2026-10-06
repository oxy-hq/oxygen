import { Info } from "lucide-react";
import { useMemo } from "react";
import ChartCard from "./ChartCard";
import type { TraceChartsProps } from "./types";
import {
  useAnalyticsRunsChartOptions,
  useAutomationRunsChartOptions,
  useDurationChartOptions,
  useTokensChartOptions
} from "./useChartOptions";
import { aggregateByDuration, aggregateByTime, calculateStats } from "./utils";

export default function TraceCharts({ traces, total, isLoading }: TraceChartsProps) {
  // The charts are drawn from one capped page of the result set. Past the cap
  // every figure here is a count of the newest slice, not of the view, and the
  // headline numbers are the ones a reader quotes — so say which it is.
  const shown = traces?.length ?? 0;
  const capped = total !== undefined && total > shown && shown > 0;

  const timeBuckets = useMemo(() => aggregateByTime(traces ?? []), [traces]);

  const durationBuckets = useMemo(() => aggregateByDuration(traces ?? []), [traces]);

  const stats = useMemo(() => calculateStats(traces), [traces]);

  const automationRunsChartOptions = useAutomationRunsChartOptions(timeBuckets);
  const analyticsRunsChartOptions = useAnalyticsRunsChartOptions(timeBuckets);
  const durationChartOptions = useDurationChartOptions(durationBuckets);
  const tokensChartOptions = useTokensChartOptions(timeBuckets);

  return (
    // One band. The sentence that limits the figures is its first line, so
    // it is read before the numbers it qualifies and cannot be scrolled apart
    // from them.
    <div className='mb-4 overflow-hidden rounded-lg border'>
      {capped && (
        <p
          className='flex items-center gap-2 border-b bg-muted/40 px-4 py-2 text-sm'
          data-testid='traces-charts-capped'
        >
          <Info className='size-3.5 shrink-0 text-muted-foreground' aria-hidden />
          <span>
            Charts and totals cover the newest{" "}
            <span className='font-medium tabular-nums'>{shown.toLocaleString()}</span> of{" "}
            <span className='font-medium tabular-nums'>{total.toLocaleString()}</span> traces in
            this view.
          </span>
        </p>
      )}
      <div className='grid grid-cols-4 divide-x'>
        <ChartCard
          label='Automation runs'
          value={stats.automationRuns}
          options={automationRunsChartOptions}
          isLoading={isLoading}
        />
        <ChartCard
          label='Analytics runs'
          value={stats.analyticsRuns}
          options={analyticsRunsChartOptions}
          isLoading={isLoading}
        />
        <ChartCard
          label='Average execution time'
          value={stats.avgDuration}
          options={durationChartOptions}
          isLoading={isLoading}
        />
        <ChartCard
          label='Total tokens used'
          value={stats.totalTokens.toLocaleString()}
          options={tokensChartOptions}
          isLoading={isLoading}
        />
      </div>
    </div>
  );
}

// Re-export types for external use
export type { TraceChartsProps } from "./types";
