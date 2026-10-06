import { Skeleton } from "@/components/ui/shadcn/skeleton";
import { useAppClientErrors } from "@/hooks/api/customApps/useCustomApps";
import type { ClientError } from "@/types/apps";
import { type LogWindowHours, windowPhrase } from "../functionLogs";

/**
 * Browser errors, grouped by stack.
 *
 * One row is one distinct fault, not one occurrence — the same bug in a render
 * loop fires thousands of times and is still one thing to fix. The counts carry
 * "how bad", the row carries "what".
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
      <p className='text-muted-foreground text-xs' data-testid='admin-app-errors-error'>
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
    <div className='space-y-2' data-testid='admin-app-errors-list'>
      {errors.map((e: ClientError) => (
        <ErrorGroup key={e.stack_hash} error={e} />
      ))}
    </div>
  );
};

const ErrorGroup = ({ error }: { error: ClientError }) => (
  <details
    className='rounded-md border bg-muted/20 px-3 py-2'
    data-testid={`admin-app-errors-group-${error.stack_hash}`}
  >
    <summary className='cursor-pointer list-none text-xs'>
      <span className='font-medium text-destructive'>{error.error_name}</span>
      <span className='ml-1.5 text-muted-foreground'>{error.message}</span>
      <span className='ml-1.5 text-muted-foreground tabular-nums'>
        · {error.occurrences.toLocaleString()}× across {error.sessions.toLocaleString()} session
        {error.sessions === 1 ? "" : "s"}
      </span>
    </summary>
    <div className='mt-2 space-y-1.5'>
      <p className='text-muted-foreground text-xs'>
        {error.path && <span className='font-mono'>{error.path}</span>}
        {error.kind === "unhandledrejection" && <span> · unhandled rejection</span>}
        <span> · last seen {error.last_seen}</span>
      </p>
      {/* Saying so matters: a still-minified stack looks like a resolved one
          until you try to open the file, and that is the moment a reader is
          misled about what they are looking at. */}
      {!error.stack_resolved && error.stack && (
        <p className='text-muted-foreground text-xs' data-testid='admin-app-errors-unresolved'>
          Frames are unresolved — no source map was published for this build.
        </p>
      )}
      {error.stack && (
        <pre className='max-h-60 overflow-auto whitespace-pre-wrap break-words rounded bg-background p-2 font-mono text-muted-foreground text-xs'>
          {error.stack}
        </pre>
      )}
    </div>
  </details>
);
