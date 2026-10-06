import type React from "react";
import { useMemo } from "react";
import EChart from "@/components/Echarts/EChart";
import { cn } from "@/libs/shadcn/utils";
import useTheme from "@/stores/useTheme";
import type { ApiKeyUsageDay } from "@/types/apiKey";
import { fillUsageDays, usageTotals } from "../../activity";
import { usageChartOption } from "./usageChartOption";

const n = (v: number) => v.toLocaleString("en-US");

/** A legend entry that is also the 30-day total, so the key and the number are one thing. */
const LegendTotal: React.FC<{ swatch: string; value: number; label: string; testId: string }> = ({
  swatch,
  value,
  label,
  testId
}) => (
  <span className='inline-flex items-center gap-1.5' data-testid={testId}>
    <span className={cn("inline-block size-2 rounded-xs", swatch)} aria-hidden />
    <span className='font-medium text-foreground tabular-nums'>{n(value)}</span>
    <span className='text-muted-foreground'>{label}</span>
  </span>
);

/** The chart's data, readable without hovering and by screen readers. */
const UsageTable: React.FC<{ days: ApiKeyUsageDay[] }> = ({ days }) => (
  <table className='sr-only'>
    <caption>Requests per day, last 30 days</caption>
    <thead>
      <tr>
        <th>Day</th>
        <th>Requests</th>
        <th>4xx</th>
        <th>5xx</th>
      </tr>
    </thead>
    <tbody>
      {days.map((d) => (
        <tr key={d.day}>
          <td>{d.day}</td>
          <td>{d.requests}</td>
          <td>{d.errors_4xx}</td>
          <td>{d.errors_5xx}</td>
        </tr>
      ))}
    </tbody>
  </table>
);

const UsageSection: React.FC<{ usage: ApiKeyUsageDay[] }> = ({ usage }) => {
  const days = useMemo(() => fillUsageDays(usage), [usage]);
  const totals = useMemo(() => usageTotals(days), [days]);
  const theme = useTheme((s) => s.theme);
  const option = useMemo(() => usageChartOption(days, theme), [days, theme]);

  return (
    <section className='flex flex-col gap-2' data-testid='api-key-activity-usage'>
      <h3 className='font-medium text-xs'>Requests, last 30 days</h3>
      {totals.requests === 0 ? (
        <p className='text-muted-foreground text-xs'>No requests in the last 30 days.</p>
      ) : (
        <>
          <div className='flex flex-wrap gap-x-4 gap-y-1 text-xs'>
            <LegendTotal
              swatch='bg-chart-seq-4'
              value={totals.ok}
              label='OK'
              testId='api-key-activity-usage-ok'
            />
            <LegendTotal
              swatch='bg-warning'
              value={totals.errors4xx}
              label='4xx'
              testId='api-key-activity-usage-4xx'
            />
            <LegendTotal
              swatch='bg-destructive'
              value={totals.errors5xx}
              label='5xx'
              testId='api-key-activity-usage-5xx'
            />
          </div>
          <EChart option={option} height={64} />
          <UsageTable days={days} />
        </>
      )}
    </section>
  );
};

export default UsageSection;
