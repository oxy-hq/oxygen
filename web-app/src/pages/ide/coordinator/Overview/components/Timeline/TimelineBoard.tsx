import type React from "react";
import { cn } from "@/libs/shadcn/utils";
import { JOB_TYPE, JOB_TYPES, RUN_STATUS } from "../../../components/constants";
import type { NormalizedRun } from "../../../components/runModel";
import { formatRelative } from "../../../components/utils";

/**
 * Status-board alternative to the timeline — one cell per run, grouped by
 * type. Trades the time axis for density; cheaper to scan "is anything red".
 */
export const TimelineBoard: React.FC<{
  runs: NormalizedRun[];
  onSelect: (runId: string) => void;
}> = ({ runs, onSelect }) => (
  <div className='flex flex-col'>
    {JOB_TYPES.map((type) => {
      const meta = JOB_TYPE[type];
      const laneRuns = runs.filter((r) => r.jobType === type);
      return (
        <div
          key={type}
          className='flex items-start gap-2 border-border border-b px-2 py-2 last:border-b-0'
        >
          <div className='flex w-24 shrink-0 items-center gap-1.5'>
            <meta.icon className={cn("h-3.5 w-3.5", meta.fg)} />
            <span className='font-medium text-xs'>{meta.short}</span>
          </div>
          <div className='flex min-w-0 flex-1 flex-wrap gap-1'>
            {laneRuns.length === 0 ? (
              <span className='text-muted-foreground text-xs'>No runs</span>
            ) : (
              laneRuns.map((run) => (
                <button
                  key={run.runId}
                  type='button'
                  onClick={() => onSelect(run.runId)}
                  title={`${run.title} — ${RUN_STATUS[run.status].label} · ${formatRelative(
                    run.startedAt
                  )}`}
                  className={cn(
                    "h-4 w-4 opacity-90 transition-all hover:opacity-100 hover:ring-2 hover:ring-ring",
                    RUN_STATUS[run.status].bg,
                    run.live && "animate-pulse"
                  )}
                />
              ))
            )}
          </div>
        </div>
      );
    })}
  </div>
);
