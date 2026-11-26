import { LayoutGrid, Rows3 } from "lucide-react";
import type React from "react";
import { useMemo, useState } from "react";
import { useNavigate } from "react-router-dom";
import { cn } from "@/libs/shadcn/utils";
import {
  JOB_TYPES,
  RUN_STATUS,
  type RunStatus,
  rangeMs,
  type TimeRange
} from "../../../components/constants";
import { Segmented } from "../../../components/Filters";
import type { NormalizedRun } from "../../../components/runModel";
import { useCoordinatorRoutes } from "../../../components/useCoordinatorRoutes";
import type { MissingSlot } from "../../useOverviewModel";
import { TimelineBoard } from "./TimelineBoard";
import { TimelineLane } from "./TimelineLane";

type View = "timeline" | "board";

const LEGEND: RunStatus[] = ["done", "running", "failed", "suspended", "cancelled"];
const AXIS_TICKS = 6;

/** Builds evenly spaced axis labels across the visible window. */
const useAxisTicks = (t0Ms: number, spanMs: number, useDate: boolean) =>
  useMemo(() => {
    const ticks: { pct: number; label: string }[] = [];
    for (let i = 0; i <= AXIS_TICKS; i++) {
      const t = new Date(t0Ms + (spanMs * i) / AXIS_TICKS);
      ticks.push({
        pct: (i / AXIS_TICKS) * 100,
        label: useDate
          ? t.toLocaleDateString(undefined, { month: "short", day: "numeric" })
          : t.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" })
      });
    }
    return ticks;
  }, [t0Ms, spanMs, useDate]);

/**
 * Hero timeline — swimlanes grouped by job type along a shared time axis,
 * with a "now" marker and a status-board toggle. The Overview's centerpiece.
 */
export const Timeline: React.FC<{
  runs: NormalizedRun[];
  missingSlots: MissingSlot[];
  range: TimeRange;
}> = ({ runs, missingSlots, range }) => {
  const navigate = useNavigate();
  const routes = useCoordinatorRoutes();
  const [view, setView] = useState<View>("timeline");

  const nowMs = Date.now();
  const rawSpan = rangeMs(range);
  const t0Ms = nowMs - rawSpan;
  // A sliver of future space so the "now" marker isn't flush to the edge.
  const spanMs = rawSpan * 1.06;
  const nowPct = (rawSpan / spanMs) * 100;
  const ticks = useAxisTicks(t0Ms, spanMs, rawSpan > 48 * 3600 * 1000);

  const onSelect = (runId: string) => navigate(routes.RUN_DETAIL(runId));
  const onSelectMiss = (scheduleId: string) => navigate(routes.JOB_DETAIL(scheduleId));

  return (
    <div data-testid='coordinator-timeline' className='border border-border bg-card'>
      <div className='flex items-center justify-between border-border border-b px-3 py-2'>
        <div className='flex items-center gap-2'>
          <h3 className='font-semibold text-sm'>Timeline</h3>
          <span className='text-muted-foreground text-xs'>{runs.length} runs</span>
        </div>
        <div className='flex items-center gap-3'>
          <div className='hidden items-center gap-2.5 sm:flex'>
            {LEGEND.map((s) => (
              <span key={s} className='flex items-center gap-1 text-muted-foreground text-xs'>
                <span className={cn("h-2 w-2 ", RUN_STATUS[s].bg)} />
                {RUN_STATUS[s].label}
              </span>
            ))}
            <span className='flex items-center gap-1 text-muted-foreground text-xs'>
              <span className='inline-block h-2 w-2 border border-warning border-dashed' />
              Missing
            </span>
          </div>
          <Segmented<View>
            value={view}
            onChange={setView}
            options={[
              { value: "timeline", label: <Rows3 className='h-3.5 w-3.5' /> },
              { value: "board", label: <LayoutGrid className='h-3.5 w-3.5' /> }
            ]}
          />
        </div>
      </div>

      {runs.length === 0 && missingSlots.length === 0 ? (
        <p className='px-3 py-10 text-center text-muted-foreground text-sm'>
          No runs in this window — widen the time range or clear the type filter.
        </p>
      ) : view === "board" ? (
        <TimelineBoard runs={runs} onSelect={onSelect} />
      ) : (
        <div className='relative pt-5'>
          {JOB_TYPES.map((type) => (
            <TimelineLane
              key={type}
              jobType={type}
              runs={runs.filter((r) => r.jobType === type)}
              misses={missingSlots.filter((m) => m.jobType === type)}
              t0Ms={t0Ms}
              spanMs={spanMs}
              nowMs={nowMs}
              onSelect={onSelect}
              onSelectMiss={onSelectMiss}
            />
          ))}
          {/* "now" marker — overlay inset to the track region (past the labels).
              The container's `pt-5` reserves a strip above the lanes so the
              "now" pill sits in its own row instead of stacking on top of the
              first lane's run rectangles (which were only visible on hover
              before because the pill's solid bg-primary covered them). */}
          <div className='pointer-events-none absolute inset-y-0 right-0 left-24'>
            <div
              className='absolute inset-y-0 border-primary border-l-2 border-dashed'
              style={{ left: `${nowPct}%` }}
            >
              <span className='absolute top-0 -translate-x-1/2 rounded-b bg-primary px-1 py-0.5 font-medium text-primary-foreground text-xs'>
                now
              </span>
            </div>
          </div>
          {/* Time axis */}
          <div className='flex border-border border-t'>
            <div className='w-24 shrink-0 border-border border-r' />
            <div className='relative h-6 flex-1'>
              {ticks.map((tick) => (
                <span
                  key={tick.pct}
                  className='absolute top-1 -translate-x-1/2 text-muted-foreground text-xs tabular-nums'
                  style={{ left: `${tick.pct}%` }}
                >
                  {tick.label}
                </span>
              ))}
            </div>
          </div>
        </div>
      )}
    </div>
  );
};
