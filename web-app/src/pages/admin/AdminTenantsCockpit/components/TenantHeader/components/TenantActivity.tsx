import { History, ShieldAlert } from "lucide-react";
import { Button } from "@/components/ui/shadcn/button";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/shadcn/popover";
import { cn } from "@/libs/shadcn/utils";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { AuditEvent } from "@/types/audit";

/** `member.role.updated` → "member role updated". The stored ids are versioned and
 *  dotted; a human reading an incident timeline wants a sentence fragment. */
function humanAction(action: string): string {
  return action.replace(/\./g, " ");
}

function relative(iso: string): string {
  const then = new Date(iso).getTime();
  if (Number.isNaN(then)) return "";
  const mins = Math.round((Date.now() - then) / 60000);
  if (mins < 1) return "just now";
  if (mins < 60) return `${mins}m ago`;
  const hrs = Math.round(mins / 60);
  if (hrs < 24) return `${hrs}h ago`;
  return `${Math.round(hrs / 24)}d ago`;
}

/**
 * Who last touched this tenant, in the header.
 *
 * Reads the same `audit_events` rows the staff membership writes now produce — the
 * feed and the audit trail are one source, so an action that isn't recorded doesn't
 * appear here either. That is the correct coupling: a timeline assembled from
 * something looser would show activity the audit log can't account for.
 */
export function TenantActivity({
  events,
  isPending,
  isError
}: {
  events: AuditEvent[];
  isPending: boolean;
  /** The read failed — most likely a 403, since `/admin/audit` needs `view_audit`. */
  isError: boolean;
}) {
  const latest = events[0];
  return (
    <Popover>
      <PopoverTrigger asChild>
        <Button
          variant='ghost'
          size='sm'
          className='h-7 gap-1.5 px-2 text-muted-foreground'
          data-testid='admin-tenant-activity-trigger'
        >
          <History className='size-3.5' />
          <span className='text-xs'>
            {isPending || isError
              ? "Activity"
              : latest
                ? relative(latest.created_at)
                : "No activity"}
          </span>
        </Button>
      </PopoverTrigger>

      <PopoverContent align='end' className='w-80 p-0' data-testid='admin-tenant-activity'>
        <p className='border-b px-3 py-2 text-[10px] text-muted-foreground uppercase tracking-[0.16em]'>
          Recent activity
        </p>
        {isPending ? (
          <p className='px-3 py-4 text-muted-foreground text-xs'>Loading…</p>
        ) : isError ? (
          /* Distinguish "cannot see" from "nothing happened". `/admin/audit` requires
             `view_audit`, so a role without it gets a 403 — rendering that as an empty
             feed would state, falsely, that this tenant has no history. */
          <p className='px-3 py-4 text-muted-foreground text-xs'>
            Activity is unavailable — this needs the audit capability.
          </p>
        ) : events.length === 0 ? (
          <p className='px-3 py-4 text-muted-foreground text-xs'>
            Nothing recorded for this tenant yet.
          </p>
        ) : (
          <ul className='max-h-72 divide-y overflow-auto'>
            {events.map((e) => (
              <li key={e.id} className='px-3 py-2'>
                <div className='flex items-baseline justify-between gap-2'>
                  <span className='truncate font-medium text-xs'>{humanAction(e.action)}</span>
                  <span className='shrink-0 text-[10px] text-muted-foreground tabular-nums'>
                    {relative(e.created_at)}
                  </span>
                </div>
                <div className='mt-0.5 flex items-center gap-1 text-[10px] text-muted-foreground'>
                  <span className='truncate'>{e.actor_email}</span>
                  {e.target_label && <span className='truncate'>→ {e.target_label}</span>}
                  {/* An action taken while impersonating is the one an incident review
                      cares about most, so it is marked rather than left to be inferred
                      from the actor's address. */}
                  {e.via_global_override && (
                    <span
                      className={cn(
                        "ml-auto flex shrink-0 items-center gap-0.5",
                        ADMIN_TONE.warn.text
                      )}
                    >
                      <ShieldAlert className='size-3' />
                      acting
                    </span>
                  )}
                </div>
              </li>
            ))}
          </ul>
        )}
      </PopoverContent>
    </Popover>
  );
}
