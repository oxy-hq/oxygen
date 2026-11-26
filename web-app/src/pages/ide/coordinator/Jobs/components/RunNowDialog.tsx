import { Play } from "lucide-react";
import type React from "react";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle
} from "@/components/ui/shadcn/alert-dialog";
import { buttonVariants } from "@/components/ui/shadcn/utils/button-variants";
import { useRunScheduleNow } from "@/hooks/api/schedules/useSchedules";
import type { Schedule } from "@/types/schedule";

interface Props {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  schedule: Schedule | null;
}

/**
 * Confirm an out-of-band manual run. A run-now triggers one execution
 * immediately and does NOT advance the cron cadence — the next scheduled
 * slot still fires on time.
 */
const RunNowDialog: React.FC<Props> = ({ open, onOpenChange, schedule }) => {
  const runNow = useRunScheduleNow();

  const confirm = () => {
    if (!schedule) return;
    runNow.mutate(schedule.id, { onSettled: () => onOpenChange(false) });
  };

  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent className={`${DESIGN_REFERENCE} bg-popover sm:max-w-md`}>
        <AlertDialogHeader>
          <AlertDialogTitle>Run "{schedule?.name}" now?</AlertDialogTitle>
          <AlertDialogDescription>
            Triggers one immediate run. The cron cadence is unaffected — the next scheduled run
            still fires on time. The run is tagged as manually triggered in the run history.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <AlertDialogAction
            data-testid='coordinator-run-now-confirm'
            onClick={confirm}
            disabled={runNow.isPending}
            className={buttonVariants({ variant: "default" })}
          >
            <Play className='h-4 w-4' />
            Run now
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
};

export default RunNowDialog;
