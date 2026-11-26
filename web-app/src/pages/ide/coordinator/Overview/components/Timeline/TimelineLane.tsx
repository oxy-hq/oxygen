import type React from "react";
import { useMemo } from "react";
import { cn } from "@/libs/shadcn/utils";
import { JOB_TYPE, type JobType, RUN_STATUS } from "../../../components/constants";
import type { NormalizedRun } from "../../../components/runModel";
import { formatDurationMs } from "../../../components/utils";
import type { MissingSlot } from "../../useOverviewModel";
import { packRows } from "./packRows";

/** Vertical pitch of one packed sub-row, in px (timeline positioning only). */
const ROW_H = 24;

/**
 * One swimlane of the timeline — all runs of a single job type, packed into
 * non-overlapping sub-rows and positioned along the shared time axis.
 * Missing-slot markers cut through the full lane height so an absent run is
 * as readable as a failure.
 */
export const TimelineLane: React.FC<{
  jobType: JobType;
  runs: NormalizedRun[];
  misses: MissingSlot[];
  t0Ms: number;
  spanMs: number;
  nowMs: number;
  onSelect: (runId: string) => void;
  onSelectMiss: (scheduleId: string) => void;
}> = ({ jobType, runs, misses, t0Ms, spanMs, nowMs, onSelect, onSelectMiss }) => {
  const meta = JOB_TYPE[jobType];
  const { placed, rowCount } = useMemo(() => packRows(runs, nowMs), [runs, nowMs]);
  const laneHeight = rowCount * ROW_H + 8;

  return (
    <div className='flex border-border border-b last:border-b-0'>
      <div className='flex w-24 shrink-0 items-center gap-1.5 border-border border-r px-2 py-1.5'>
        <meta.icon className={cn("h-3.5 w-3.5", meta.fg)} />
        <span className='font-medium text-xs'>{meta.short}</span>
        <span className='ml-auto text-muted-foreground text-xs tabular-nums'>{runs.length}</span>
      </div>
      <div className='relative min-w-0 flex-1' style={{ height: laneHeight }}>
        {/* Missing slots — vertical dashed warning markers, behind the bars. */}
        {misses.map((miss) => {
          const left = ((miss.atMs - t0Ms) / spanMs) * 100;
          if (left < 0 || left > 100) return null;
          return (
            <button
              key={`${miss.scheduleId}-${miss.atMs}`}
              type='button'
              onClick={() => onSelectMiss(miss.scheduleId)}
              title={`Missing run: ${miss.scheduleName} — expected ${new Date(miss.atMs).toLocaleString()}`}
              className={cn(
                "absolute inset-y-0 w-1.5 -translate-x-1/2 border-warning border-l border-dashed",
                "hover:bg-warning/10"
              )}
              style={{ left: `${left}%` }}
            />
          );
        })}
        {/* Actual runs. */}
        {placed.map(({ run, row, startMs, endMs }) => {
          const visStart = Math.max(startMs, t0Ms);
          const visEnd = Math.max(Math.min(endMs, nowMs), visStart);
          const left = ((visStart - t0Ms) / spanMs) * 100;
          const width = ((visEnd - visStart) / spanMs) * 100;
          const status = RUN_STATUS[run.status];
          return (
            <button
              key={run.runId}
              type='button'
              onClick={() => onSelect(run.runId)}
              title={`${run.title} — ${status.label} · ${formatDurationMs(endMs - startMs)}`}
              className={cn(
                "absolute h-4 min-w-1.5 opacity-90 transition-all",
                "hover:z-10 hover:opacity-100 hover:ring-2 hover:ring-ring",
                status.bg,
                run.live && "animate-pulse"
              )}
              style={{
                left: `${left}%`,
                width: `${Math.max(width, 0.4)}%`,
                top: row * ROW_H + 4
              }}
            />
          );
        })}
      </div>
    </div>
  );
};
