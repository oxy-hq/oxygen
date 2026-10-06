import { Info } from "lucide-react";
import {
  HEALTH_HISTORY_DAYS,
  useWorkspaceHealthHistory
} from "@/hooks/api/workspaceHealth/useWorkspaceHealthHistory";
import type { WorkspaceHealthHistory } from "@/services/api/workspaceHealth";
import { AdminAsync } from "../../../components/AdminAsync";
import { AdminSectionLabel } from "../../../components/AdminSectionLabel";
import { AdminStatusPill } from "../../../components/AdminStatusPill";
import { workspaceHealthTone } from "../../../components/workspaceHealthTone";
import {
  formatSpan,
  type HealthInterval,
  lengthOf,
  summarize,
  toIntervals
} from "./healthIntervals";

const when = new Intl.DateTimeFormat(undefined, {
  month: "short",
  day: "numeric",
  hour: "2-digit",
  minute: "2-digit"
});

/**
 * How this workspace's status has moved: the stretches it spent in each
 * status, newest first, under one sentence that totals them.
 *
 * The state above this says what is wrong now. This says whether that is new —
 * the fourth time this month reads differently from the first — and how long
 * each one lasted. A row names the dimensions that were failing when the
 * stretch began; the reasons behind them are kept only for the current state.
 */
export function HealthHistory({
  workspaceId,
  labelOf
}: {
  workspaceId: string;
  /** A dimension's display name. A name the console no longer knows comes
   *  back as written. */
  labelOf: (dimension: string) => string;
}) {
  const history = useWorkspaceHealthHistory(workspaceId);
  return (
    <section
      className='space-y-3 rounded-lg border border-border/60 bg-card p-6'
      data-testid='admin-workspace-health-history'
    >
      <AdminSectionLabel>History (last {HEALTH_HISTORY_DAYS} days)</AdminSectionLabel>
      <AdminAsync query={history} noun='health history' rows={2}>
        {(data) => <HistoryBody history={data} labelOf={labelOf} />}
      </AdminAsync>
    </section>
  );
}

function HistoryBody({
  history,
  labelOf
}: {
  history: WorkspaceHealthHistory;
  labelOf: (dimension: string) => string;
}) {
  // One clock reading for the summary and every row, so they cannot disagree
  // about how long the stretch still going on has lasted.
  const now = Date.now();
  const intervals = toIntervals(history, now);
  return (
    <div className='space-y-3 text-xs'>
      <p data-testid='admin-workspace-health-history-summary'>{summarize(history, now)}</p>
      {intervals.length > 0 && (
        // One grid; the heading and every stretch are subgrids of it, so the
        // times and durations are columns that can be read down.
        <div className='grid grid-cols-[auto_auto_auto_minmax(0,1fr)] gap-x-6'>
          <div className='col-span-full grid grid-cols-subgrid border-border/60 border-b pb-1.5 text-[10px] text-muted-foreground uppercase tracking-[0.16em]'>
            <span>Status</span>
            <span>From</span>
            <span className='text-right'>For</span>
            <span>Failing when it began</span>
          </div>
          {intervals.map((interval) => (
            <IntervalRow
              key={`${interval.start}:${interval.status}`}
              interval={interval}
              now={now}
              labelOf={labelOf}
            />
          ))}
        </div>
      )}
      {history.truncated && (
        <p
          className='flex items-start gap-1.5 text-muted-foreground'
          data-testid='admin-workspace-health-history-truncated'
        >
          <Info className='mt-0.5 size-3 shrink-0' aria-hidden />
          Only the {history.transitions.length} most recent changes are listed.
        </p>
      )}
    </div>
  );
}

function IntervalRow({
  interval,
  now,
  labelOf
}: {
  interval: HealthInterval;
  now: number;
  labelOf: (dimension: string) => string;
}) {
  return (
    <div
      className='col-span-full grid grid-cols-subgrid items-baseline border-border/60 border-b py-1.5 last:border-b-0'
      data-testid='admin-workspace-health-history-row'
    >
      <span>
        <AdminStatusPill tone={workspaceHealthTone(interval.status)} label={interval.status} />
      </span>
      <span className='text-muted-foreground tabular-nums'>
        {interval.beganBeforeWindow ? "before " : ""}
        {when.format(interval.start)}
      </span>
      <span className='text-right text-sm tabular-nums'>
        {formatSpan(lengthOf(interval, now))}
        {interval.end === null && <span className='text-muted-foreground text-xs'> so far</span>}
      </span>
      <span className='min-w-0 break-words'>
        {interval.failures.length > 0 ? (
          interval.failures.map(labelOf).join(", ")
        ) : (
          <span className='text-muted-foreground'>—</span>
        )}
      </span>
    </div>
  );
}
