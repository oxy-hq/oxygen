import { cn } from "@/libs/shadcn/utils";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { PeriodComparison } from "./invocationStats";
import { formatChange } from "./invocationStats";

const pct = (rate: number | null) => (rate == null ? "" : ` (${(rate * 100).toFixed(1)}%)`);
const count = (n: number, atLeast: boolean) => `${atLeast ? "at least " : ""}${n.toLocaleString()}`;

/**
 * This window's count beside the equal window before it: how many ran, how many
 * failed, and the change. The counts are the server's, over the whole window, not
 * the capped page. Only a failure count above zero is coloured.
 */
export const InvocationsSummary = ({
  comparison,
  windowLabel,
  previousPending
}: {
  comparison: PeriodComparison;
  /** "24h", "7 days". */
  windowLabel: string;
  /** The window before has not answered yet. */
  previousPending: boolean;
}) => {
  const { current, previous, totalChange, failureRate } = comparison;
  const before = `the ${windowLabel} before`;
  return (
    <dl
      className='grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-0.5 text-xs'
      data-testid='admin-app-invocations-summary'
    >
      <dt className='text-muted-foreground'>Invocations</dt>
      <dd>
        <span
          className='font-medium text-sm tabular-nums'
          data-testid='admin-app-invocations-total'
        >
          {count(current.total, current.atLeast)}
        </span>
        <span className='text-muted-foreground'>
          {" "}
          in the last {windowLabel}
          {previousPending
            ? ""
            : ` · ${totalChange == null ? "" : `${formatChange(totalChange)} on `}${count(previous.total, previous.atLeast)} ${before}`}
        </span>
      </dd>
      <dt className='text-muted-foreground'>Failed</dt>
      <dd>
        <span
          className={cn(
            "font-medium text-sm tabular-nums",
            current.failed > 0 && ADMIN_TONE.danger.text
          )}
          data-testid='admin-app-invocations-failed'
        >
          {current.failed.toLocaleString()}
        </span>
        <span className='text-muted-foreground'>
          {pct(failureRate.current)}
          {previousPending
            ? ""
            : ` · ${previous.failed.toLocaleString()}${pct(failureRate.previous)} ${before}`}
        </span>
      </dd>
    </dl>
  );
};
