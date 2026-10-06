import { ClipboardCopy, ListTree } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/shadcn/button";
import { cn } from "@/libs/shadcn/utils";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import { CopyableId } from "@/pages/admin/components/CopyableId";
import type { AppIssue } from "@/services/api/appIssues";
import type { CustomApp } from "@/types/apps";
import { relativeTime } from "../../Activity/relativeTime";
import { describeLastFailure, issueBrief } from "../issueBrief";

/**
 * One distinct failure of one function.
 *
 * The collapsed row answers the two questions an operator has before opening
 * anything: how much, and is it still happening on what users are served. The
 * second is the dot — the live build has had this failure, or it has not —
 * derived from the rows, never set by anyone.
 */
export const IssueRow = ({
  app,
  issue,
  windowDays,
  onOpenFunction
}: {
  app: CustomApp;
  issue: AppIssue;
  windowDays: number;
  onOpenFunction: (name: string) => void;
}) => {
  const tone = issue.on_live_build ? ADMIN_TONE.danger : ADMIN_TONE.muted;

  const copyBrief = async () => {
    try {
      await navigator.clipboard.writeText(issueBrief(app, issue, windowDays));
      toast.success("Issue copied");
    } catch (err) {
      console.error("Could not copy the issue", err);
      toast.error("Could not copy the issue");
    }
  };

  return (
    <li>
      <details
        className='rounded-md border border-border bg-card'
        data-testid={`admin-app-issues-row-${issue.function_name}-${issue.fingerprint}`}
      >
        <summary className='flex cursor-pointer list-none flex-wrap items-center gap-x-2 gap-y-0.5 px-3 py-2 text-xs'>
          <span className={cn("size-1.5 shrink-0 rounded-full", tone.dot)} aria-hidden />
          <span className='font-medium'>{issue.function_name}</span>
          <span className='text-muted-foreground tabular-nums'>
            {issue.occurrences.toLocaleString()}×
          </span>
          <span className='text-muted-foreground'>last {relativeTime(issue.last_seen)}</span>
          <span className={cn("ml-auto", tone.text)} data-testid='admin-app-issues-row-standing'>
            {issue.on_live_build ? "on the live build" : "not on the live build"}
          </span>
        </summary>
        <div className='space-y-2 border-t px-3 py-2'>
          {/* Wrapped, never truncated: this is the most important text in the
              section and the one a narrow column would cut first. */}
          <pre
            className='max-h-60 overflow-auto whitespace-pre-wrap break-words rounded bg-muted/40 p-2 font-mono text-xs'
            data-testid='admin-app-issues-row-error'
          >
            {describeLastFailure(issue)}
          </pre>
          <p className='flex flex-wrap items-center gap-x-2 gap-y-0.5 text-muted-foreground text-xs'>
            <span>first seen {relativeTime(issue.first_seen)}</span>
            <span>
              · {issue.builds} build{issue.builds === 1 ? "" : "s"}
              {issue.last.build_id ? `, last on ${issue.last.build_id}` : ""}
            </span>
            <span className='flex items-center'>
              · fingerprint
              <CopyableId value={issue.fingerprint} head={16} />
            </span>
            <span className='flex items-center'>
              · invocation
              <CopyableId value={issue.last.invocation_id} />
            </span>
          </p>
          <div className='flex flex-wrap gap-1.5'>
            <Button
              variant='outline'
              size='sm'
              className='h-6 gap-1.5 px-2 text-xs'
              onClick={() => onOpenFunction(issue.function_name)}
              data-testid='admin-app-issues-row-open-function'
            >
              <ListTree className='size-3' />
              Invocations
            </Button>
            <Button
              variant='outline'
              size='sm'
              className='h-6 gap-1.5 px-2 text-xs'
              onClick={copyBrief}
              data-testid='admin-app-issues-row-copy'
            >
              <ClipboardCopy className='size-3' />
              Copy for an agent
            </Button>
          </div>
        </div>
      </details>
    </li>
  );
};
