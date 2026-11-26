import { AlertTriangle, CheckCircle2, History } from "lucide-react";
import type React from "react";
import { useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import { cn } from "@/libs/shadcn/utils";
import type { Schedule } from "@/types/schedule";
import { BackfillDialog } from "../../../components/BackfillDialog";
import { isSystemSchedule } from "../../../components/constants";
import { formatTimestamp } from "../../../components/utils";

/**
 * The reliability side of a job — last error, missed-run coverage, and the
 * backfill entry point. A missing run is silent; this card makes the gap as
 * visible as a failure.
 *
 * Airway jobs are date-window (not cron-slot) backfilled and own "coverage" in
 * the backfill-ranges gantt, so for them this collapses to just schedule health
 * (last fire status): the missed-occurrences block and the cron backfill button
 * are hidden and the title drops "& coverage". System-managed kinds hide the
 * button for a different reason — see `isSystemSchedule` — but keep the
 * missed-occurrences block, which is still true and worth seeing.
 */
export const JobHealthCard: React.FC<{
  schedule: Schedule;
  canManage: boolean;
  isAirway?: boolean;
}> = ({ schedule, canManage, isAirway = false }) => {
  const [backfillOpen, setBackfillOpen] = useState(false);
  // System-managed kinds have no past occurrence to replay — the server
  // rejects a backfill for them, so don't offer the button. Run now is the
  // operation they do have.
  const canBackfill = !isAirway && !isSystemSchedule(schedule);

  return (
    <div className='border border-border bg-card'>
      <div className='flex items-center justify-between border-border border-b px-3 py-2'>
        <h3 className='font-semibold text-sm'>
          {isAirway ? "Schedule health" : "Health & coverage"}
        </h3>
        {canManage && canBackfill && (
          <Button
            size='sm'
            variant='outline'
            data-testid='coordinator-backfill-button'
            onClick={() => setBackfillOpen(true)}
          >
            <History className='h-4 w-4' />
            Backfill
          </Button>
        )}
      </div>
      <div className='flex flex-col gap-3 p-3'>
        {schedule.last_error ? (
          <div className='flex gap-2 bg-destructive/10 px-3 py-2'>
            <AlertTriangle className='mt-0.5 h-4 w-4 shrink-0 text-destructive' />
            <div className='min-w-0'>
              <p className='font-medium text-destructive text-sm'>Last fire failed</p>
              <p className='break-words text-muted-foreground text-xs'>{schedule.last_error}</p>
            </div>
          </div>
        ) : (
          <div className='flex items-center gap-2 text-sm'>
            <CheckCircle2 className='h-4 w-4 text-success' />
            <span>No fire errors — the scheduler is firing this job cleanly.</span>
          </div>
        )}

        {!isAirway && (
          <div className='border border-border px-3 py-2'>
            <div className='flex items-baseline justify-between'>
              <span className='text-muted-foreground text-xs uppercase tracking-wide'>
                Missed occurrences
              </span>
              <span
                className={cn(
                  "font-semibold text-lg tabular-nums",
                  schedule.missed_runs > 0 ? "text-warning" : "text-foreground"
                )}
              >
                {schedule.missed_runs}
              </span>
            </div>
            <p className='mt-1 text-muted-foreground text-xs'>
              {schedule.missed_runs > 0
                ? `Slots skipped during scheduler downtime (last detected ${formatTimestamp(
                    schedule.last_missed_at
                  )}). Policy is run-once-then-resume — only the first missed slot fires automatically${
                    canBackfill ? "; backfill the rest." : "."
                  }`
                : "Every scheduled slot has fired on time."}
            </p>
          </div>
        )}
      </div>

      {canBackfill && (
        <BackfillDialog open={backfillOpen} onOpenChange={setBackfillOpen} schedule={schedule} />
      )}
    </div>
  );
};
