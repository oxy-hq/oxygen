import { X } from "lucide-react";
import { Button } from "@/components/ui/shadcn/button";
import { Spinner } from "@/components/ui/shadcn/spinner";
import type { AnomalyStatus } from "@/types/metricAnomalies";

/**
 * Batch action bar for the selected anomalies. Rendered only when something is
 * selected, so the table's own layout is untouched at rest.
 *
 * Counts anomalies only, deliberately. A bucket count would be a guess: the
 * write names events, and the server resolves buckets the list response may
 * have capped away — so any number here could be lower than what the button
 * actually writes. The toast reports the real figure once the server has it.
 */
export default function BulkActionBar({
  eventCount,
  actionable,
  pendingStatus,
  busy,
  onApply,
  onClear
}: {
  eventCount: number;
  /** How many of the selection each action would actually move. An action
   *  that would move none is not offered: the per-row buttons already hide
   *  themselves that way, and a batch that writes nothing comes back as
   *  "no longer in this view" — which is wrong about rows sitting right
   *  there in the target status. */
  actionable: Record<"acknowledged" | "dismissed", number>;
  /** The status this bar is writing, or `undefined` — cosmetic, it only places
   *  the spinner. Disabling is `busy`'s job: a *row* write leaves this
   *  undefined while very much being in flight. */
  pendingStatus: AnomalyStatus | undefined;
  /** Any write is in flight — including one started by a row. Without this the
   *  bar stayed live under a row's Ack, and a second click raced two writes
   *  over a row belonging to both. */
  busy: boolean;
  onApply: (status: "acknowledged" | "dismissed") => void;
  onClear: () => void;
}) {
  return (
    <div className='mb-3 flex items-center justify-between gap-2 border border-border-subtle bg-field px-3 py-2'>
      <span className='text-sm'>
        <span className='font-medium'>{eventCount}</span>{" "}
        {eventCount === 1 ? "anomaly" : "anomalies"} selected
      </span>
      <div className='flex items-center gap-1'>
        {actionable.acknowledged > 0 && (
          <Button
            size='sm'
            variant='outline'
            disabled={busy}
            onClick={() => onApply("acknowledged")}
          >
            {pendingStatus === "acknowledged" && <Spinner className='size-4' />}
            {/* The count the button will actually move, not the selection
                size: rows already acknowledged are skipped, and a bar reading
                "4 selected" over a toast reading "3 acknowledged" leaves the
                user to guess which one was left and why. */}
            Ack {actionable.acknowledged} selected
          </Button>
        )}
        {actionable.dismissed > 0 && (
          <Button size='sm' variant='outline' disabled={busy} onClick={() => onApply("dismissed")}>
            {pendingStatus === "dismissed" && <Spinner className='size-4' />}
            Dismiss {actionable.dismissed} selected
          </Button>
        )}
        {/* Not `disabled={busy}`, unlike the two actions beside it. `busy`
            covers every write on the screen including a single row's Ack, and
            clearing writes nothing — freezing it would leave a mis-selection
            stuck on screen until an unrelated row action finished. */}
        <Button
          size='icon'
          variant='ghost'
          className='size-8 text-muted-foreground'
          onClick={onClear}
          aria-label='Clear selection'
        >
          <X className='size-4' />
        </Button>
      </div>
    </div>
  );
}
