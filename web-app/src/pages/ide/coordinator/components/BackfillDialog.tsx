import { CalendarRange, Info } from "lucide-react";
import type React from "react";
import { useMemo, useState } from "react";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import { Button } from "@/components/ui/shadcn/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle
} from "@/components/ui/shadcn/dialog";
import { Input } from "@/components/ui/shadcn/input";
import { Label } from "@/components/ui/shadcn/label";
import { useBackfillSchedule } from "@/hooks/api/schedules/useSchedules";
import type { Schedule } from "@/types/schedule";
import { Segmented } from "./Filters";
import { cronCountBetween, describeCron } from "./utils";

/** Format a Date for an <input type="datetime-local"> value. */
const toLocalInput = (d: Date): string => {
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(
    d.getHours()
  )}:${pad(d.getMinutes())}`;
};

interface Props {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  schedule: Schedule | null;
}

/**
 * Backfill confirmation flow. A missing run is silent — this is where an
 * operator detects the gap and fills it. The guardrails: state the run
 * *count* (not just the range), persist the concurrency hint, and make the
 * per-run logical date explicit. Confirm calls the backfill endpoint;
 * seeded runs flow through the normal queue tagged `trigger=backfill`.
 */
export const BackfillDialog: React.FC<Props> = ({ open, onOpenChange, schedule }) => {
  const [startStr, setStartStr] = useState(() =>
    toLocalInput(new Date(Date.now() - 24 * 60 * 60 * 1000))
  );
  const [endStr, setEndStr] = useState(() => toLocalInput(new Date()));
  const [concurrency, setConcurrency] = useState("sequential");
  const backfill = useBackfillSchedule();

  const start = new Date(startStr);
  const end = new Date(endStr);
  const rangeValid = !Number.isNaN(start.getTime()) && !Number.isNaN(end.getTime()) && end > start;

  const runCount = useMemo(() => {
    if (!schedule || !rangeValid) return 0;
    return cronCountBetween(schedule.cron_expr, schedule.timezone, start, end);
  }, [schedule, start, end, rangeValid]);

  if (!schedule) return null;

  const confirm = () => {
    if (!schedule || !rangeValid) return;
    backfill.mutate(
      {
        id: schedule.id,
        input: {
          from: start.toISOString(),
          to: end.toISOString(),
          concurrency
        }
      },
      { onSuccess: () => onOpenChange(false) }
    );
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className={`${DESIGN_REFERENCE} sm:max-w-lg`}>
        <DialogHeader>
          <DialogTitle>Backfill: {schedule.name}</DialogTitle>
          <DialogDescription>
            {schedule.cron_expr} · {describeCron(schedule.cron_expr)} · {schedule.timezone}
          </DialogDescription>
        </DialogHeader>

        <div className='flex flex-col gap-4'>
          <div className='grid grid-cols-2 gap-3'>
            <div className='flex flex-col gap-1.5'>
              <Label htmlFor='bf-start'>From</Label>
              <Input
                id='bf-start'
                type='datetime-local'
                value={startStr}
                onChange={(e) => setStartStr(e.target.value)}
              />
            </div>
            <div className='flex flex-col gap-1.5'>
              <Label htmlFor='bf-end'>To</Label>
              <Input
                id='bf-end'
                type='datetime-local'
                value={endStr}
                onChange={(e) => setEndStr(e.target.value)}
              />
            </div>
          </div>

          {/* Blast radius — the count, not just the range. */}
          <div className='flex items-center gap-2 bg-muted px-3 py-2.5'>
            <CalendarRange className='h-4 w-4 shrink-0 text-muted-foreground' />
            {rangeValid ? (
              <p className='text-sm'>
                This will trigger{" "}
                <span className='font-semibold text-foreground'>
                  {runCount} run{runCount === 1 ? "" : "s"}
                </span>
                {runCount > 50 && (
                  <span className='text-warning'> — large backfill, throttle carefully</span>
                )}
                .
              </p>
            ) : (
              <p className='text-destructive text-sm'>End must be after start.</p>
            )}
          </div>

          <div className='flex flex-col gap-1.5'>
            <Label>Concurrency</Label>
            <Segmented
              value={concurrency}
              onChange={setConcurrency}
              options={[
                { value: "sequential", label: "Sequential" },
                { value: "2", label: "2 at a time" },
                { value: "5", label: "5 at a time" },
                { value: "all", label: "All at once" }
              ]}
            />
            <p className='text-muted-foreground text-xs'>
              Throttling avoids hammering source systems during a large backfill.
            </p>
          </div>

          <div className='flex gap-2 border border-border px-3 py-2 text-muted-foreground text-xs'>
            <Info className='mt-0.5 h-3.5 w-3.5 shrink-0' />
            <span>
              Each run receives its <strong>original scheduled time</strong> as the logical date,
              and is tagged <strong>backfill</strong> — shown distinctly from scheduled runs in the
              timeline and run log.
            </span>
          </div>
        </div>

        <DialogFooter>
          <Button variant='outline' onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button
            data-testid='coordinator-backfill-confirm'
            onClick={confirm}
            disabled={!rangeValid || runCount === 0 || backfill.isPending}
          >
            {backfill.isPending
              ? "Queueing…"
              : `Run backfill${runCount > 0 ? ` (${runCount})` : ""}`}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};
