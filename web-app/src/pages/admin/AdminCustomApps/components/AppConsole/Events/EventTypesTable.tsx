import { ChevronRight } from "lucide-react";
import { Fragment, useState } from "react";
import { cn } from "@/libs/shadcn/utils";
import { timeAgo } from "@/libs/utils/date";
import { AdminCount } from "@/pages/admin/components/AdminCount";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { EventRole, EventTypeRow } from "./eventsView";
import { formatWait } from "./eventsView";
import { ListenerRows } from "./ListenerRows";

const ROLE_LABEL: Record<EventRole, string> = {
  publishes: "Publishes",
  listens: "Listens",
  both: "Publishes · listens"
};

/**
 * One row per event type, a listener's queue counts summed across it, the listeners
 * one click down. The shape of Encore's Pub/Sub page: the name and who is involved on
 * the left, then counts that fade at zero, so a type with dead letters is the only
 * thing on a quiet list that reads.
 */
export const EventTypesTable = ({ orgSlug, rows }: { orgSlug: string; rows: EventTypeRow[] }) => {
  const [open, setOpen] = useState<ReadonlySet<string>>(new Set());
  const toggle = (type: string) =>
    setOpen((prev) => {
      const next = new Set(prev);
      if (next.has(type)) next.delete(type);
      else next.add(type);
      return next;
    });

  return (
    <table className='w-full text-xs' data-testid='admin-app-events-types'>
      <thead>
        <tr className='border-border/60 border-b text-left text-muted-foreground'>
          <th className='py-2 pr-3 pl-8 font-medium'>Event type</th>
          <th className='px-3 py-2 text-right font-medium'>Pending</th>
          <th className='px-3 py-2 text-right font-medium'>Retrying</th>
          <th className='px-3 py-2 text-right font-medium'>Dead</th>
          <th className='px-3 py-2 text-right font-medium'>Held</th>
          <th className='px-3 py-2 text-right font-medium max-md:hidden'>Oldest wait</th>
          <th className='py-2 pr-3 pl-3 text-right font-medium max-md:hidden'>Last published</th>
        </tr>
      </thead>
      <tbody>
        {rows.map((row) => {
          const expanded = open.has(row.type);
          const wait = formatWait(row.counts.oldestPendingSeconds);
          return (
            <Fragment key={row.type}>
              <tr
                data-testid={`admin-app-events-type-${row.type}`}
                className='cursor-pointer border-border/60 border-b hover:bg-muted/30'
                onClick={() => toggle(row.type)}
              >
                <td className='py-2 pr-3 pl-2'>
                  <div className='flex min-w-0 items-start gap-1.5'>
                    <button
                      type='button'
                      aria-expanded={expanded}
                      aria-label={`${expanded ? "Hide" : "Show"} listeners of ${row.type}`}
                      onClick={(e) => {
                        e.stopPropagation();
                        toggle(row.type);
                      }}
                      className='mt-px rounded text-muted-foreground hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring'
                    >
                      <ChevronRight
                        className={cn("size-3.5 transition-transform", expanded && "rotate-90")}
                      />
                    </button>
                    <div className='min-w-0'>
                      <div className='truncate font-medium font-mono'>{row.type}</div>
                      <div className='truncate text-muted-foreground'>
                        {ROLE_LABEL[row.role]} · {row.listeners.length}{" "}
                        {row.listeners.length === 1 ? "listener" : "listeners"}
                        {row.schemaRevision !== null ? ` · schema r${row.schemaRevision}` : ""}
                      </div>
                    </div>
                  </div>
                </td>
                <td className='px-3 py-2 text-right'>
                  <AdminCount value={row.counts.pending} />
                </td>
                <td className='px-3 py-2 text-right'>
                  <AdminCount value={row.counts.retrying} />
                </td>
                <td className='px-3 py-2 text-right'>
                  <AdminCount
                    value={row.counts.dead}
                    className={row.counts.dead > 0 ? ADMIN_TONE.danger.text : undefined}
                  />
                </td>
                <td className='px-3 py-2 text-right'>
                  <AdminCount
                    value={row.counts.held}
                    className={row.counts.held > 0 ? ADMIN_TONE.warn.text : undefined}
                  />
                </td>
                <td className='px-3 py-2 text-right tabular-nums max-md:hidden'>
                  {wait ?? <span className='text-muted-foreground/60'>—</span>}
                </td>
                <td className='py-2 pr-3 pl-3 text-right text-muted-foreground tabular-nums max-md:hidden'>
                  {row.lastPublishedAt ? (
                    <span title={new Date(row.lastPublishedAt).toLocaleString()}>
                      {timeAgo(row.lastPublishedAt)}
                    </span>
                  ) : (
                    <span className='text-muted-foreground/60'>never</span>
                  )}
                </td>
              </tr>
              {expanded ? (
                <tr className='border-border/60 border-b bg-muted/20'>
                  <td colSpan={7} className='px-3 py-2 pl-8'>
                    <ListenerRows orgSlug={orgSlug} type={row.type} listeners={row.listeners} />
                  </td>
                </tr>
              ) : null}
            </Fragment>
          );
        })}
      </tbody>
    </table>
  );
};
