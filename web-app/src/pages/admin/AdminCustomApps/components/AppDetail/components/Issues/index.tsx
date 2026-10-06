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
      isEmpty={(data) => data.issues.length === 0}
      empty={
        <p className='text-muted-foreground text-xs' data-testid='admin-app-issues-empty'>
          No function has failed in production in the last {ISSUE_WINDOW_DAYS} days. An app with no
          Oxy Functions looks the same.
        </p>
      }
    >
      {(data) => (
        <div className='space-y-2'>
          <ul className='flex flex-col gap-1.5' data-testid='admin-app-issues-list'>
            {data.issues.map((issue) => (
              <IssueRow
                key={`${issue.function_name}:${issue.fingerprint}`}
                app={app}
                issue={issue}
                windowDays={data.window_days}
                onOpenFunction={onOpenFunction}
              />
            ))}
          </ul>
          {data.truncated && (
            <p className='text-muted-foreground text-xs' data-testid='admin-app-issues-truncated'>
              Showing the {data.issues.length} most recently seen. More distinct failures happened
              in the last {data.window_days} days than are listed.
            </p>
          )}
        </div>
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
