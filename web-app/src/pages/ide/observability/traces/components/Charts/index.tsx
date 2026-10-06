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
    <div className='mb-4'>
      <div className='grid grid-cols-4 gap-4'>
        <ChartCard
          title='Automation Runs'
          value={`${stats.automationRuns} Automation Runs`}
          subtitle=''
          options={automationRunsChartOptions}
          isLoading={isLoading}
        />

        <ChartCard
          title='Analytics Runs'
          value={`${stats.analyticsRuns} Analytics Runs`}
          subtitle=''
          options={analyticsRunsChartOptions}
          isLoading={isLoading}
        />

        <ChartCard
          title='Duration'
          value={`${stats.avgDuration} Average Execution Time`}
          subtitle=''
          options={durationChartOptions}
          isLoading={isLoading}
        />

        <ChartCard
          title='Tokens'
          value={`${stats.totalTokens.toLocaleString()} Total Tokens Used`}
          subtitle=''
          options={tokensChartOptions}
          isLoading={isLoading}
        />
      </div>
      {capped && (
        <p className='mt-2 px-3 text-muted-foreground text-xs' data-testid='traces-charts-capped'>
          Charts and totals cover the newest {shown.toLocaleString()} of {total.toLocaleString()}{" "}
          traces in this view.
        </p>
      )}
    </div>
  );
}

// Re-export types for external use
export type { TraceChartsProps } from "./types";
