import { ChevronRight } from "lucide-react";
import { useState } from "react";
import { Link } from "react-router-dom";
import { cn } from "@/libs/shadcn/utils";
import { timeAgo } from "@/libs/utils/date";
import ROUTES from "@/libs/utils/routes";
import { AdminStatusPill } from "@/pages/admin/components/AdminStatusPill";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import { workspaceHealthTone } from "@/pages/admin/components/workspaceHealthTone";
import { DIMENSION_LABEL, type HealthCause } from "../groupByCause";

/**
 * One incident: the failure stated once, in full, with the workspaces it took down
 * underneath.
 *
 * The reason is the most important text on this page and gets the room to be read —
 * `whitespace-pre-wrap break-words` in mono. It used to be a table cell competing with
 * three other columns, so a 206-character connector error ran off the right edge of a
 * 1440px viewport with no way to see the end of it. An operator cannot act on a failure
 * they cannot finish reading.
 */
export const CauseCard = ({ cause, index }: { cause: HealthCause; index: number }) => {
  // The first cause is open: worst-first ordering means it is the one being worked on.
  const [open, setOpen] = useState(index === 0);
  const tone = ADMIN_TONE[workspaceHealthTone(cause.status)];
  const count = cause.workspaces.length;

  return (
    <section
      data-testid='admin-workspace-health-cause'
      data-status={cause.status}
      className='overflow-hidden rounded-lg border border-border/60 bg-card'
    >
      <div className='flex items-start gap-3 border-border/60 border-b p-4'>
        <span className={cn("mt-1 size-2 shrink-0 rounded-full", tone.dot)} aria-hidden />
        <div className='min-w-0 flex-1 space-y-2'>
          <p
            data-testid='admin-workspace-health-cause-reason'
            className='whitespace-pre-wrap break-words font-mono text-foreground text-xs leading-relaxed'
          >
            {cause.reason}
          </p>
          <div className='flex flex-wrap items-center gap-1.5'>
            <AdminStatusPill tone={workspaceHealthTone(cause.status)} label={cause.status} />
            {cause.dimensions.map((d) => (
              <span
                key={d}
                className='rounded-sm border border-border/60 px-1.5 py-0.5 text-[10px] text-muted-foreground uppercase tracking-[0.14em]'
              >
                {DIMENSION_LABEL[d]}
              </span>
            ))}
          </div>
        </div>
      </div>

      <button
        type='button'
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
        data-testid='admin-workspace-health-cause-toggle'
        className='flex w-full items-center gap-1.5 px-4 py-2 text-left text-muted-foreground text-xs transition-colors hover:bg-muted/40 hover:text-foreground'
      >
        <ChevronRight className={cn("size-3 transition-transform", open && "rotate-90")} />
        <span className='tabular-nums'>
          {count} workspace{count === 1 ? "" : "s"} affected
        </span>
      </button>

      {open ? (
        <ul className='divide-y divide-border/60 border-border/60 border-t'>
          {cause.workspaces.map((ws) => (
            <li key={ws.workspace_id}>
              <Link
                to={`${ROUTES.ADMIN.WORKSPACE_DETAIL(ws.workspace_id)}?tab=health`}
                data-testid='admin-workspace-health-cause-workspace'
                className='flex items-baseline justify-between gap-3 px-4 py-2 transition-colors hover:bg-muted/40'
              >
                <span className='min-w-0'>
                  <span className='font-medium text-xs hover:underline'>
                    {ws.workspace_name ?? "Unknown workspace"}
                  </span>
                  {ws.org_name ? (
                    <span className='ml-2 text-muted-foreground text-xs'>{ws.org_name}</span>
                  ) : null}
                </span>
                <span className='shrink-0 text-muted-foreground text-xs tabular-nums'>
                  {ws.checked_at ? (
                    <span title={new Date(ws.checked_at).toLocaleString()}>
                      {timeAgo(ws.checked_at)}
                    </span>
                  ) : (
                    "—"
                  )}
                </span>
              </Link>
            </li>
          ))}
        </ul>
      ) : null}
    </section>
  );
};
