import { ChevronRight, ClipboardCopy, ListTree } from "lucide-react";
import { useId, useState } from "react";
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
 * One distinct failure of one function, as an entry in the issues ledger.
 *
 * A row is a subgrid of the list's columns, so its figures sit under the
 * headings and line up with every other row's. Its identity is the pair the
 * pager quotes — function over fingerprint — at the left edge, where the
 * operator arriving from Slack looks first. Whether the build production
 * serves now has had the failure sits beside the fingerprint, on the left
 * with the rest of what the row *is*, so it cannot collide with the figures
 * however narrow the dossier is docked.
 *
 * The whole row opens it: the toggle button's hit area is stretched over the
 * row, and the fingerprint's own copy button sits above that.
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
  const [open, setOpen] = useState(false);
  const detailId = useId();

  return (
    <div
      className='col-span-full grid grid-cols-subgrid border-border/60 border-b last:border-b-0'
      data-testid={`admin-app-issues-row-${issue.function_name}-${issue.fingerprint}`}
    >
      <div className='relative col-span-full grid grid-cols-subgrid items-start px-4 py-2 text-xs transition-colors hover:bg-muted/40'>
        <div className='min-w-0'>
          <button
            type='button'
            aria-expanded={open}
            aria-controls={detailId}
            onClick={() => setOpen((was) => !was)}
            className='flex max-w-full items-center gap-1.5 text-left font-medium after:absolute after:inset-0 focus-visible:outline-none focus-visible:after:ring-1 focus-visible:after:ring-ring'
            data-testid='admin-app-issues-row-toggle'
          >
            <ChevronRight
              className={cn(
                "size-3 shrink-0 text-muted-foreground transition-transform",
                open && "rotate-90"
              )}
              aria-hidden
            />
            <span className='truncate'>{issue.function_name}</span>
          </button>
          <div className='ml-4.5 flex flex-wrap items-center gap-x-3'>
            <CopyableId
              value={issue.fingerprint}
              full
              group={4}
              className='relative z-10 -ml-1 text-muted-foreground'
            />
            <Standing live={issue.on_live_build} />
          </div>
        </div>
        <span className='text-right text-sm tabular-nums'>
          {issue.occurrences.toLocaleString()}
        </span>
        <span className='text-right text-muted-foreground tabular-nums'>
          {relativeTime(issue.last_seen)}
        </span>
      </div>
      {open && (
        <IssueDetail
          id={detailId}
          app={app}
          issue={issue}
          windowDays={windowDays}
          onOpenFunction={onOpenFunction}
        />
      )}
    </div>
  );
};

/** Has the build production serves now had this failure — derived from the
 *  rows, never set by anyone. Only the answer that needs someone carries the
 *  status colour; a failure the live build has not had is history, and is
 *  said in plain grey. */
const Standing = ({ live }: { live: boolean }) => (
  <span
    className={cn(
      "inline-flex items-center gap-1.5",
      live ? cn("font-medium", ADMIN_TONE.danger.text) : "text-muted-foreground"
    )}
    data-testid='admin-app-issues-row-standing'
  >
    {live && <span className={cn("size-1.5 rounded-full", ADMIN_TONE.danger.dot)} aria-hidden />}
    {live ? "On the live build" : "Not on the live build"}
  </span>
);

const IssueDetail = ({
  id,
  app,
  issue,
  windowDays,
  onOpenFunction
}: {
  id: string;
  app: CustomApp;
  issue: AppIssue;
  windowDays: number;
  onOpenFunction: (name: string) => void;
}) => {
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
    <div id={id} className='col-span-full space-y-3 px-4 pb-3 pl-8.5 text-xs'>
      <div className='space-y-1'>
        <p className='text-muted-foreground'>Last failure</p>
        {/* Wrapped, never truncated: this is the most important text in the
            section and the one a narrow column would cut first. */}
        <pre
          className='max-h-60 overflow-auto whitespace-pre-wrap break-words rounded bg-muted/40 px-3 py-2 font-mono text-xs leading-relaxed'
          data-testid='admin-app-issues-row-error'
        >
          {describeLastFailure(issue)}
        </pre>
      </div>
      <dl className='grid grid-cols-[auto_1fr] items-baseline gap-x-4 gap-y-1'>
        <dt className='text-muted-foreground'>Last invocation</dt>
        <dd className='min-w-0'>
          <CopyableId value={issue.last.invocation_id} full className='-ml-1' />
        </dd>
        <dt className='text-muted-foreground'>First seen</dt>
        <dd>{relativeTime(issue.first_seen)}</dd>
        <dt className='text-muted-foreground'>Builds</dt>
        <dd className='tabular-nums'>
          {issue.builds}
          {issue.last.build_id && (
            <>
              , last on <span className='font-mono'>{issue.last.build_id}</span>
            </>
          )}
        </dd>
      </dl>
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
  );
};
