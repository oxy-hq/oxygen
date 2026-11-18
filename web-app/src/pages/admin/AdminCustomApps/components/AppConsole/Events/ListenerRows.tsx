import { timeAgo } from "@/libs/utils/date";
import { AdminCount } from "@/pages/admin/components/AdminCount";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import { CommandSnippet } from "@/pages/admin/components/CommandSnippet";
import type { AppEventListenerHealth } from "@/services/api/appEvents";
import { formatWait } from "./eventsView";

/**
 * The functions listening to one type, each with its own queue. A listener with dead
 * deliveries gets the `oxyc` command that queues them again, ready to copy: the page
 * itself does not redeliver, because the bulk redeliver spans every environment and
 * this page shows one.
 */
export const ListenerRows = ({
  orgSlug,
  type,
  listeners
}: {
  /** Named in the command: an operator copying it is rarely inside that org's repo, and
   *  `oxyc` would otherwise take the org of whatever repo it runs in. */
  orgSlug: string;
  type: string;
  listeners: AppEventListenerHealth[];
}) => {
  if (listeners.length === 0) {
    return (
      <p
        className='py-1 text-muted-foreground'
        data-testid={`admin-app-events-listeners-${type}-empty`}
      >
        Nothing listens in this environment. Publishes are kept, not delivered.
      </p>
    );
  }
  return (
    <div className='flex flex-col' data-testid={`admin-app-events-listeners-${type}`}>
      <div className='grid grid-cols-[minmax(0,1fr)_repeat(4,4.5rem)_6rem] gap-x-3 pb-1 font-medium text-[10px] text-muted-foreground uppercase tracking-[0.14em] max-md:hidden'>
        <span>Listener</span>
        <span className='text-right'>Pending</span>
        <span className='text-right'>Retrying</span>
        <span className='text-right'>Dead</span>
        <span className='text-right'>Held</span>
        <span className='text-right'>Last success</span>
      </div>
      {listeners.map((l) => (
        <div key={`${l.listener}@${l.environment}`} className='flex flex-col gap-1 py-1'>
          <div className='grid grid-cols-[minmax(0,1fr)_repeat(4,4.5rem)_6rem] items-baseline gap-x-3 max-md:grid-cols-[minmax(0,1fr)_repeat(4,3rem)]'>
            <span className='min-w-0 truncate font-mono' title={l.listener}>
              {l.listener}
              {l.oldestPendingAgeSeconds !== null ? (
                <span className='ml-2 font-sans text-muted-foreground'>
                  waiting {formatWait(l.oldestPendingAgeSeconds)}
                </span>
              ) : null}
            </span>
            <AdminCount value={l.pending} className='text-right' />
            <AdminCount value={l.retrying} className='text-right' />
            <AdminCount
              value={l.dead}
              className={l.dead > 0 ? `text-right ${ADMIN_TONE.danger.text}` : "text-right"}
            />
            <AdminCount
              value={l.held}
              className={l.held > 0 ? `text-right ${ADMIN_TONE.warn.text}` : "text-right"}
            />
            <span className='text-right text-muted-foreground tabular-nums max-md:hidden'>
              {l.lastSuccessAt ? timeAgo(l.lastSuccessAt) : "never"}
            </span>
          </div>
          {l.dead > 0 ? (
            <CommandSnippet
              className='max-w-xl'
              command={`oxyc events redeliver --org ${orgSlug} --to ${l.listener} --status dead --dry-run`}
            />
          ) : null}
        </div>
      ))}
    </div>
  );
};
