import type { UsageReport } from "@/types/usageReport";
import { summaryCounts } from "../summaryCounts";
import { formatPeriod } from "../utils";

/**
 * The top of the report: which week, and what happened in it, in a sentence.
 *
 * The sentence is the hero on purpose. It is the one the server also puts at the top of
 * the Monday email, and it already carries the numbers a row of stat cards would repeat —
 * so the counts under it are one quiet line, not a second headline.
 */
export function UsageSummary({ report }: { report: UsageReport }) {
  const { summary } = report;
  const period = formatPeriod(report.period_start, report.period_end);

  return (
    <section className='space-y-1' data-testid='admin-usage-report-summary'>
      {/* The sentences keep a reading measure; the counts below may run the page's width,
          so six of them still sit on one line. */}
      <div className='max-w-2xl space-y-1'>
        {period ? (
          <p className='text-muted-foreground text-xs' data-testid='admin-usage-report-period'>
            {period}
          </p>
        ) : null}
        <p className='font-medium text-sm' data-testid='admin-usage-report-headline'>
          {summary.headline}
        </p>
        <p className='text-muted-foreground text-xs' data-testid='admin-usage-report-comparison'>
          {summary.comparison}
        </p>
      </div>
      <ul
        className='flex flex-wrap gap-x-5 gap-y-1 pt-2 text-muted-foreground text-xs tabular-nums'
        data-testid='admin-usage-report-counts'
      >
        {summaryCounts(summary).map((count) => (
          <li key={count.id} data-testid={`admin-usage-report-count-${count.id}`}>
            <span className='text-foreground'>{count.value}</span> {count.label}
          </li>
        ))}
      </ul>
    </section>
  );
}
