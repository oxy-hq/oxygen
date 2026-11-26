import { ChevronRight } from "lucide-react";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger
} from "@/components/ui/shadcn/collapsible";
import { cn } from "@/libs/shadcn/utils";
import { timeAgo } from "@/libs/utils/date";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { PublishAttempt } from "@/types/appPublishAttempts";
import { shortBuildId } from "../../../AppLifecycle/lifecycleCopy";
import { environmentLabel } from "../Secrets/environmentLabel";
import { failedAttempts, publisherLabel } from "./publishStages";
import { StagesBar } from "./StagesBar";

/** How many failures show before the rest fold behind a disclosure. */
export const FAILED_SHOWN = 3;

/**
 * The publishes that failed, newest first, each with the stage it failed at and
 * the error in full. A failed publish leaves no build, so the ledger below cannot
 * show it; this is the only place it is seen. Renders nothing when the attempts
 * read has no failure in it.
 */
export const FailedPublishes = ({ attempts }: { attempts: PublishAttempt[] | undefined }) => {
  const failed = failedAttempts(attempts);
  if (failed.length === 0) return null;
  const shown = failed.slice(0, FAILED_SHOWN);
  const older = failed.slice(FAILED_SHOWN);

  return (
    <section className='flex flex-col gap-2' data-testid='admin-app-failed-publishes'>
      <h3 className='font-medium text-xs'>
        Failed publishes <span className='text-muted-foreground tabular-nums'>{failed.length}</span>
      </h3>
      <ul className='flex flex-col gap-1'>
        {shown.map((a) => (
          <FailedPublish key={a.id} attempt={a} />
        ))}
      </ul>
      {older.length > 0 && (
        <Collapsible data-testid='admin-app-failed-publishes-older'>
          <CollapsibleTrigger className='group flex items-center gap-1.5 text-left text-muted-foreground text-xs hover:text-foreground'>
            <ChevronRight className='size-3 shrink-0 transition-transform group-data-[state=open]:rotate-90' />
            {older.length} older {older.length === 1 ? "failure" : "failures"}
          </CollapsibleTrigger>
          <CollapsibleContent className='pt-1'>
            <ul className='flex flex-col gap-1'>
              {older.map((a) => (
                <FailedPublish key={a.id} attempt={a} />
              ))}
            </ul>
          </CollapsibleContent>
        </Collapsible>
      )}
    </section>
  );
};

const FailedPublish = ({ attempt }: { attempt: PublishAttempt }) => {
  const who = publisherLabel(attempt);
  const testId = `admin-app-failed-publish-${attempt.id}`;
  return (
    <li
      className='flex min-w-0 flex-col gap-1.5 border border-border bg-card px-3 py-2'
      data-testid={testId}
    >
      <div className='flex flex-wrap items-center gap-x-2 gap-y-0.5 text-xs'>
        <span className='font-mono' title={attempt.build_id}>
          {shortBuildId(attempt.build_id)}
        </span>
        <span
          className={cn(
            " px-1.5 py-px font-medium text-[10px]",
            ADMIN_TONE.danger.bg,
            ADMIN_TONE.danger.text
          )}
          data-testid={`${testId}-stage`}
        >
          {attempt.failed_stage ? `Failed at ${attempt.failed_stage}` : "Failed"}
        </span>
        <span
          className='text-muted-foreground'
          title={new Date(attempt.started_at).toLocaleString()}
        >
          {timeAgo(attempt.started_at)}
          {who ? ` · ${who}` : ""}
          {` · to ${environmentLabel(attempt.environment).toLowerCase()}`}
        </span>
      </div>
      {attempt.error && (
        <p
          className='whitespace-pre-wrap break-words font-mono text-xs'
          data-testid={`${testId}-error`}
        >
          {attempt.error}
        </p>
      )}
      <StagesBar
        stages={attempt.stages}
        failedStage={attempt.failed_stage}
        testId={`${testId}-stages`}
      />
    </li>
  );
};
