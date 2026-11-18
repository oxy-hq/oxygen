import { ChevronRight } from "lucide-react";
import { useState } from "react";
import { Link } from "react-router-dom";
import { cn } from "@/libs/shadcn/utils";
import { timeAgo } from "@/libs/utils/date";
import ROUTES from "@/libs/utils/routes";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import { workspaceHealthTone } from "@/pages/admin/components/workspaceHealthTone";
import { DIMENSION_LABEL, type HealthCause } from "../groupByCause";

/**
 * One incident, as one row of the causes list: the failure, what kind of check found
 * it, and how many workspaces it took down. The workspaces and the failure in full are
 * one click down.
 *
 * **The failure is clamped, not cut.** It used to be printed whole in every card, and a
 * page of five causes was five walls of mono: three of them the same metric-tree error
 * with a different UUID. Closed, a row shows the first two lines, which is where a
 * failure says what it is. Open, it prints in full with `whitespace-pre-wrap`, so the
 * end of a 206-character connector error is still never off the edge of the screen.
 */
export const CauseCard = ({ cause }: { cause: HealthCause; index?: number }) => {
  const [open, setOpen] = useState(false);
  const tone = ADMIN_TONE[workspaceHealthTone(cause.status)];
  const count = cause.workspaces.length;

  return (
    <section data-testid='admin-workspace-health-cause' data-status={cause.status}>
      <button
        type='button'
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
        data-testid='admin-workspace-health-cause-toggle'
        className='flex w-full items-start gap-3 px-4 py-2.5 text-left transition-colors hover:bg-muted/40 focus-visible:bg-muted/40 focus-visible:outline-none'
      >
        <ChevronRight
          className={cn(
            "mt-0.5 size-3.5 shrink-0 text-muted-foreground transition-transform",
            open && "rotate-90"
          )}
        />
        <span className={cn("mt-1.5 size-1.5 shrink-0 rounded-full", tone.dot)} aria-hidden />
        <span className='min-w-0 flex-1'>
          <span
            data-testid='admin-workspace-health-cause-reason'
            className={cn(
              "block break-words font-mono text-foreground text-xs leading-relaxed",
              open ? "whitespace-pre-wrap" : "line-clamp-2"
            )}
          >
            {cause.reason}
          </span>
          <span className='mt-1 flex flex-wrap items-center gap-1.5'>
            {cause.dimensions.map((d) => (
              <span
                key={d}
                className='rounded-sm border border-border/60 px-1.5 py-px text-[10px] text-muted-foreground uppercase tracking-[0.14em]'
              >
                {DIMENSION_LABEL[d]}
              </span>
            ))}
          </span>
        </span>
        <span className='shrink-0 text-muted-foreground text-xs tabular-nums'>
          <span className='font-medium text-foreground'>{count}</span> workspace
          {count === 1 ? "" : "s"}
        </span>
      </button>

      {open ? (
        <ul className='divide-y divide-border/60 border-border/60 border-t bg-muted/20'>
          {cause.workspaces.map((ws) => (
            <li key={ws.workspace_id}>
              <Link
                to={`${ROUTES.ADMIN.WORKSPACE_DETAIL(ws.workspace_id)}?tab=health`}
                data-testid='admin-workspace-health-cause-workspace'
                className='flex items-baseline justify-between gap-3 py-2 pr-4 pl-14 transition-colors hover:bg-muted/40'
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
