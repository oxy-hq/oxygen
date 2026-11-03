import { ExternalLink, PanelRight, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/shadcn/button";
import { cn } from "@/libs/shadcn/utils";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { AppEnvironment } from "@/services/api/customApps";
import { shortBuildId } from "../../../../AppLifecycle/lifecycleCopy";
import { relativeTime } from "../../Activity/relativeTime";
import { chosenPlanes, deletion, untilText } from "../utils";
import { OltpBranchStatus } from "./OltpBranchStatus";

/**
 * One environment. Beyond its build and owner, a branch says where it came from
 * (`parent`, `fork_at`), which planes it chose to isolate, and — for a sandbox —
 * the day it is deleted and why, so the 30-day limit on copied data is never a
 * surprise.
 */
export const EnvironmentRow = ({
  env,
  onDelete,
  onDetails
}: {
  env: AppEnvironment;
  onDelete: () => void;
  onDetails: () => void;
}) => {
  const sandbox = env.kind === "dev";
  const deletingNow = env.status === "deleting";
  const deleted = deletion(env);
  return (
    <li
      className='flex items-start gap-3 px-3 py-2 text-xs'
      data-testid={`admin-app-env-row-${env.name}`}
    >
      <div className='flex min-w-0 flex-1 flex-col gap-0.5'>
        <div className='flex items-center gap-2'>
          <span className='font-medium font-mono'>{env.name}</span>
          <span className='text-muted-foreground'>{env.kind}</span>
          {deletingNow && <span className={cn(ADMIN_TONE.warn.text)}>deleting</span>}
          {env.oltp_branch && <OltpBranchStatus branch={env.oltp_branch} name={env.name} />}
        </div>
        <span className='truncate text-muted-foreground'>
          {env.build_id ? `build ${shortBuildId(env.build_id)}` : "nothing published"}
          {env.owner?.email ? ` · ${env.owner.email}` : ""}
          {env.last_activity_at ? ` · active ${relativeTime(env.last_activity_at)}` : ""}
        </span>
        {env.parent && <BranchLine env={env} />}
        {sandbox && deleted && (
          <span className='text-muted-foreground' data-testid={`admin-app-env-deleted-${env.name}`}>
            deleted {untilText(deleted.at)} ({deleted.reason})
          </span>
        )}
      </div>
      {env.kind !== "production" && (
        <Button
          variant='ghost'
          size='sm'
          className='h-6 gap-1 px-1.5 text-xs'
          onClick={onDetails}
          data-testid={`admin-app-env-details-${env.name}`}
        >
          <PanelRight className='size-3' />
          Details
        </Button>
      )}
      {env.url && (
        <Button asChild variant='ghost' size='sm' className='h-6 gap-1 px-1.5 text-xs'>
          <a
            href={env.url}
            target='_blank'
            rel='noreferrer'
            data-testid={`admin-app-env-open-${env.name}`}
          >
            Open
            <ExternalLink className='size-3' />
          </a>
        </Button>
      )}
      {sandbox && (
        <Button
          variant='ghost'
          size='sm'
          className='h-6 gap-1 px-1.5 text-muted-foreground text-xs'
          disabled={deletingNow}
          onClick={onDelete}
          data-testid={`admin-app-env-delete-${env.name}`}
        >
          <Trash2 className='size-3' />
          Delete
        </Button>
      )}
    </li>
  );
};

/** "from staging · forked 2h ago", then a chip per plane it chose itself. */
const BranchLine = ({ env }: { env: AppEnvironment }) => {
  const chosen = chosenPlanes(env);
  return (
    <div
      className='flex flex-wrap items-center gap-1'
      data-testid={`admin-app-env-branch-${env.name}`}
    >
      <span className='text-muted-foreground'>
        from {env.parent}
        {env.fork_at ? ` · forked ${relativeTime(env.fork_at)}` : ""}
      </span>
      {env.planes_error ? (
        <span
          className={cn(ADMIN_TONE.danger.text)}
          data-testid={`admin-app-env-planes-error-${env.name}`}
        >
          planes unreadable: {env.planes_error}
        </span>
      ) : (
        chosen.map(({ plane, mode }) => (
          <span
            key={plane}
            className='rounded bg-muted px-1 font-mono text-muted-foreground'
            data-testid={`admin-app-env-plane-${env.name}-${plane}`}
          >
            {plane}={mode}
          </span>
        ))
      )}
    </div>
  );
};
