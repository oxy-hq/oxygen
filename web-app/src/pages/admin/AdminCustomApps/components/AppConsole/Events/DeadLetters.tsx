import { ChevronRight } from "lucide-react";
import { useState } from "react";
import { cn } from "@/libs/shadcn/utils";
import { timeAgo } from "@/libs/utils/date";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type {
  AppEventDead,
  AppEventDeadDelivery,
  AppEventDeadGroup
} from "@/services/api/appEvents";

/**
 * Dead deliveries grouped by how they died: ten dead letters with one cause read as one
 * problem. The error's first line is the group's headline, clamped to two lines; the
 * deliveries under it, and each one's full error, are one click down.
 */
export const DeadLetters = ({ dead }: { dead: AppEventDead }) => (
  <div className='flex flex-col divide-y divide-border/60' data-testid='admin-app-events-dead'>
    {dead.groups.map((group) => (
      <DeadGroup key={group.signature} group={group} />
    ))}
    {dead.truncated ? (
      <p className='px-3 py-2 text-muted-foreground'>
        The newest are shown; more died than one read lists.
      </p>
    ) : null}
  </div>
);

/**
 * How many tries a dead delivery had. `attemptCount` is the delivery's current budget,
 * which a redeliver resets, so a delivery that died again after one reads 0 there; the
 * attempts it carries are what actually ran. When neither says, the count is left out
 * rather than printed as "0 attempts" beside an error that reads "failed after 5".
 */
const attemptsOf = (d: AppEventDeadDelivery): number => Math.max(d.attemptCount, d.attempts.length);

const DeadGroup = ({ group }: { group: AppEventDeadGroup }) => {
  const [open, setOpen] = useState(false);
  return (
    <div data-testid='admin-app-events-dead-group'>
      <button
        type='button'
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
        className='flex w-full items-start gap-2 px-3 py-2 text-left hover:bg-muted/30 focus-visible:bg-muted/30 focus-visible:outline-none'
      >
        <ChevronRight
          className={cn(
            "mt-0.5 size-3.5 shrink-0 text-muted-foreground transition-transform",
            open && "rotate-90"
          )}
        />
        <span
          className={cn(
            "shrink-0 px-1 py-px font-medium text-[10px] tabular-nums",
            ADMIN_TONE.danger.bg,
            ADMIN_TONE.danger.text
          )}
        >
          {group.count}×
        </span>
        <span className='shrink-0 font-medium'>{group.outcome}</span>
        <span className='line-clamp-2 min-w-0 break-words font-mono text-muted-foreground'>
          {group.error || "no error recorded"}
        </span>
      </button>
      {open ? (
        <ul className='flex flex-col gap-2 px-3 pb-3 pl-9'>
          {group.deliveries.map((d) => (
            <li key={d.id} className='flex flex-col gap-0.5'>
              <div className='flex flex-wrap items-baseline gap-x-3'>
                <span className='font-mono'>{d.type}</span>
                <span className='font-mono text-muted-foreground'>key {d.key}</span>
                <span className='font-mono text-muted-foreground'>→ {d.listener}</span>
                <span className='ml-auto text-muted-foreground tabular-nums'>
                  {attemptsOf(d) > 0
                    ? `${attemptsOf(d)} ${attemptsOf(d) === 1 ? "attempt" : "attempts"} · `
                    : null}
                  died <span title={new Date(d.diedAt).toLocaleString()}>{timeAgo(d.diedAt)}</span>
                </span>
              </div>
              {d.lastError ? (
                <pre className='whitespace-pre-wrap break-words bg-muted/40 px-2 py-1 font-mono text-[11px] text-muted-foreground'>
                  {d.lastError}
                </pre>
              ) : null}
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
};
