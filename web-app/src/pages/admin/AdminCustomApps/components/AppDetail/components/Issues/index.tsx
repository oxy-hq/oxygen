import { Info } from "lucide-react";
import { ISSUE_WINDOW_DAYS, useAppIssues } from "@/hooks/api/customApps/useAppIssues";
import { cn } from "@/libs/shadcn/utils";
import { AdminAsync } from "@/pages/admin/components/AdminAsync";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { CustomApp } from "@/types/apps";
import { IssueRow } from "./components/IssueRow";
import { liveIssueCount } from "./issueBrief";

/**
 * Issues — what is wrong with this app, one row per distinct failure.
 *
 * The Functions section lists invocations and the pager quotes a fingerprint;
 * matching the two was done by eye. This groups the same rows the way the
 * pager does — per `(function, fingerprint)` — and says of each whether the
 * build production serves now has had it.
 *
 * There is nothing to set here: no status, no acknowledge, no mute. A fix that
 * shipped shows as an issue the live build has not had, and one that did not
 * work shows the opposite, without anyone having to remember to reopen it.
 */
export const Issues = ({
  app,
  onOpenFunction
}: {
  app: CustomApp;
  /** Jump to a function's invocations (`?fn=`). */
  onOpenFunction: (name: string) => void;
}) => {
  const issues = useAppIssues(app.id);

  return (
    <AdminAsync
      query={issues}
      noun='this app&rsquo;s issues'
      rows={2}
      // The list runs edge to edge and owns its gutter; the three states that
      // are not the list take the section's inset instead.
      className='mx-4 mb-4'
      isEmpty={(data) => data.issues.length === 0}
      empty={
        <p className='text-muted-foreground text-xs' data-testid='admin-app-issues-empty'>
          No function has failed in production in the last {ISSUE_WINDOW_DAYS} days. An app with no
          Oxy Functions looks the same.
        </p>
      }
    >
      {(data) => (
        <>
          {/* The one grid container: the heading row and every issue are
              subgrids of it, which is what makes the figures a column. */}
          <div
            className='grid grid-cols-[minmax(0,1fr)_auto_auto] gap-x-4'
            data-testid='admin-app-issues-list'
          >
            <div className='col-span-full grid grid-cols-subgrid border-border/60 border-b px-4 py-1.5 text-[10px] text-muted-foreground uppercase tracking-[0.16em]'>
              <span className='pl-4.5'>Function / fingerprint</span>
              <span className='text-right'>
                <span className='@lg:inline hidden'>Occurrences · </span>
                {data.window_days}d
              </span>
              <span className='text-right'>
                Last<span className='@lg:inline hidden'> seen</span>
              </span>
            </div>
            {data.issues.map((issue) => (
              <IssueRow
                key={`${issue.function_name}:${issue.fingerprint}`}
                app={app}
                issue={issue}
                windowDays={data.window_days}
                onOpenFunction={onOpenFunction}
              />
            ))}
          </div>
          {data.truncated && (
            // The foot of the list it cuts, not a paragraph somewhere after it.
            <p
              className='flex items-start gap-1.5 border-border/60 border-t bg-muted/40 px-4 py-2 text-xs'
              data-testid='admin-app-issues-truncated'
            >
              <Info className='mt-0.5 size-3 shrink-0 text-muted-foreground' aria-hidden />
              <span>
                Showing the <span className='font-medium tabular-nums'>{data.issues.length}</span>{" "}
                most recently seen. More distinct failures happened in the last {data.window_days}{" "}
                days than are listed.
              </span>
            </p>
          )}
        </>
      )}
    </AdminAsync>
  );
};

/**
 * The count in the section header, so the collapsed section still says the
 * live build is failing. Counts only issues the live build has had: one last
 * seen on a replaced build is history, and a badge for history would be lit on
 * every app that ever had a bug.
 *
 * Renders nothing at zero, like the Secrets badge beside it.
 */
export const IssuesBadge = ({ appId }: { appId: string }) => {
  const { data } = useAppIssues(appId);
  const live = data ? liveIssueCount(data.issues) : 0;
  if (live === 0) return null;
  return (
    <span
      className={cn(
        "rounded-sm px-1.5 py-px font-medium text-[10px] tabular-nums tracking-normal",
        ADMIN_TONE.danger.bg,
        ADMIN_TONE.danger.text
      )}
      data-testid='admin-app-issues-live-badge'
    >
      {live} on live build
    </span>
  );
};
