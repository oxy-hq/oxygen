import type { ReactNode } from "react";
import { AdminSectionLabel } from "../../components/AdminSectionLabel";
import { usd } from "../format";

export interface BreakdownItem {
  key: string;
  label: ReactNode;
  cost_usd: number;
  run_count: number;
  /** Fraction of the window's priced cost, or null when nothing was priced. */
  share: number | null;
}

/**
 * One dimension of spend as a ranked bar list — model, workspace, surface,
 * attendance.
 *
 * A list and not a pie: the question is "which of these is big", which is a
 * comparison of lengths along a shared baseline, and that is the one thing a
 * pie chart is bad at. It is also the only form that survives twelve models
 * without a legend.
 *
 * **A bar's width is its share of priced cost, and a bucket with no share has
 * no bar.** `share: null` means nothing in the window could be priced, so a
 * full-width bar would claim 100% of a total that does not exist and an empty
 * one would read as "this spent nothing" — both are assertions the data does
 * not support. The run count still shows, because that part *was* measured.
 */
export const BreakdownBars = ({
  title,
  items,
  emptyLabel,
  testId,
  limit = 8
}: {
  title: string;
  items: BreakdownItem[];
  emptyLabel: string;
  testId: string;
  limit?: number;
}) => {
  const shown = items.slice(0, limit);
  const rest = items.length - shown.length;

  return (
    <section className='space-y-3' data-testid={testId}>
      <AdminSectionLabel trailing={rest > 0 ? `+${rest} more` : undefined}>
        {title}
      </AdminSectionLabel>
      {shown.length === 0 ? (
        <p className='text-muted-foreground text-xs' data-testid={`${testId}-empty`}>
          {emptyLabel}
        </p>
      ) : (
        <div className='space-y-2.5'>
          {shown.map((item) => (
            <div key={item.key} className='space-y-1' data-testid={`${testId}-row`}>
              <div className='flex items-baseline justify-between gap-2 text-xs'>
                <span className='min-w-0 flex-1 truncate'>{item.label}</span>
                <span className='shrink-0 text-muted-foreground tabular-nums'>
                  {item.run_count.toLocaleString()} run{item.run_count === 1 ? "" : "s"}
                </span>
                <span className='w-16 shrink-0 text-right font-medium tabular-nums'>
                  {usd(item.cost_usd)}
                </span>
              </div>
              <div className='h-1 w-full overflow-hidden rounded-full bg-muted'>
                {item.share === null ? null : (
                  <div
                    className='h-full rounded-full bg-primary/70'
                    // A bucket with a real but tiny share still gets a visible
                    // sliver: "almost nothing" and "nothing" are different
                    // answers, and a 0px bar renders them identically.
                    style={{ width: `${Math.max(item.share * 100, item.cost_usd > 0 ? 2 : 0)}%` }}
                  />
                )}
              </div>
            </div>
          ))}
        </div>
      )}
    </section>
  );
};
