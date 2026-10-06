import { ChevronRight, Info, TriangleAlert } from "lucide-react";
import { useId, useState } from "react";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import { useAppClientErrors } from "@/hooks/api/customApps/useCustomApps";
import { cn } from "@/libs/shadcn/utils";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { ClientError } from "@/types/apps";
import { type LogWindowHours, windowPhrase } from "../functionLogs";

/**
 * Browser errors, grouped by stack.
 *
 * One row is one distinct fault, not one occurrence — the same bug in a render
 * loop fires thousands of times and is still one thing to fix. The counts carry
 * "how bad", the row carries "what": each count is a column of its own, so the
 * worst fault is found by reading down the figures rather than out of a
 * sentence per row.
 */
export const ClientErrors = ({
  orgSlug,
  appSlug,
  hours
}: {
  orgSlug: string;
  appSlug: string;
  hours: LogWindowHours;
}) => {
  const { data, isLoading, error } = useAppClientErrors(orgSlug, appSlug, hours);

  if (isLoading) return <Skeleton className='h-24 w-full' />;
  if (error) {
    return (
      <p
        className={cn("flex items-center gap-1.5 text-xs", ADMIN_TONE.danger.text)}
        data-testid='admin-app-errors-error'
      >
        <TriangleAlert className='size-3 shrink-0' aria-hidden />
        Could not read client errors.
      </p>
    );
  }
  const errors = data ?? [];
  if (errors.length === 0) {
    return (
      <p className='text-muted-foreground text-xs' data-testid='admin-app-errors-empty'>
        No uncaught browser errors in {windowPhrase(hours)}.
      </p>
    );
  }

  return (
    <div className='overflow-hidden rounded-md border'>
      {/* One grid; the heading and every fault are subgrids of it. */}
      <div
        className='grid grid-cols-[minmax(0,1fr)_auto_auto] gap-x-4'
        data-testid='admin-app-errors-list'
      >
        <div className='col-span-full grid grid-cols-subgrid border-border/60 border-b px-3 py-1.5 text-[10px] text-muted-foreground uppercase tracking-[0.16em]'>
          <span className='pl-4.5'>Error</span>
          <span className='text-right'>
            <span className='@lg:inline hidden'>Occurrences</span>
            <span className='@lg:hidden'>Count</span>
          </span>
          <span className='text-right'>Sessions</span>
        </div>
        {errors.map((e: ClientError) => (
          <ErrorGroup key={e.stack_hash} error={e} />
        ))}
      </div>
    </div>
  );
};

const ErrorGroup = ({ error }: { error: ClientError }) => {
  const [open, setOpen] = useState(false);
  const detailId = useId();

  return (
    <div
      className='col-span-full grid grid-cols-subgrid border-border/60 border-b last:border-b-0'
      data-testid={`admin-app-errors-group-${error.stack_hash}`}
    >
      <div className='relative col-span-full grid grid-cols-subgrid items-start px-3 py-2 text-xs transition-colors hover:bg-muted/40'>
        <div className='min-w-0'>
          {/* The name is not coloured: every row in this list is an error, so
              red here would say nothing the heading has not. */}
          <button
            type='button'
            aria-expanded={open}
            aria-controls={detailId}
            onClick={() => setOpen((was) => !was)}
            className='flex max-w-full items-center gap-1.5 text-left font-medium after:absolute after:inset-0 focus-visible:outline-none focus-visible:after:ring-1 focus-visible:after:ring-ring'
          >
            <ChevronRight
              className={cn(
                "size-3 shrink-0 text-muted-foreground transition-transform",
                open && "rotate-90"
              )}
              aria-hidden
            />
            <span className='truncate'>{error.error_name}</span>
          </button>
          {/* Above the row's stretched hit area, so the message can still be
              selected and copied; it is shown nowhere else. */}
          <p className='relative z-10 ml-4.5 cursor-text break-words text-muted-foreground'>
            {error.message}
          </p>
        </div>
        <span className='text-right text-sm tabular-nums'>
          {error.occurrences.toLocaleString()}
        </span>
        <span className='text-right text-sm tabular-nums'>{error.sessions.toLocaleString()}</span>
      </div>
      {open && <ErrorDetail id={detailId} error={error} />}
    </div>
  );
};

const ErrorDetail = ({ id, error }: { id: string; error: ClientError }) => (
  <div id={id} className='col-span-full space-y-2 px-3 pb-3 pl-7.5 text-xs'>
    <dl className='grid grid-cols-[auto_1fr] items-baseline gap-x-4 gap-y-1'>
      {error.path && (
        <>
          <dt className='text-muted-foreground'>Path</dt>
          <dd className='min-w-0 break-words font-mono'>{error.path}</dd>
        </>
      )}
      {error.kind === "unhandledrejection" && (
        <>
          <dt className='text-muted-foreground'>Kind</dt>
          <dd>unhandled rejection</dd>
        </>
      )}
      <dt className='text-muted-foreground'>Last seen</dt>
      <dd className='font-mono tabular-nums'>{error.last_seen}</dd>
    </dl>
    {error.stack && (
      <div className='overflow-hidden rounded bg-muted/40'>
        {/* Saying so matters: a still-minified stack looks like a resolved one
            until you try to open the file, and that is the moment a reader is
            misled about what they are looking at. It is printed on the stack
            it describes. */}
        {!error.stack_resolved && (
          <p
            className='flex items-start gap-1.5 border-border/60 border-b px-3 py-1.5'
            data-testid='admin-app-errors-unresolved'
          >
            <Info className='mt-0.5 size-3 shrink-0 text-muted-foreground' aria-hidden />
            Frames are unresolved — no source map was published for this build.
          </p>
        )}
        <pre className='max-h-60 overflow-auto whitespace-pre-wrap break-words px-3 py-2 font-mono text-muted-foreground text-xs leading-relaxed'>
          {error.stack}
        </pre>
      </div>
    )}
  </div>
);
