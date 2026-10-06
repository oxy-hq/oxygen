import { Info, type LucideIcon } from "lucide-react";
import type { ReactNode } from "react";
import { Button } from "@/components/ui/shadcn/button";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import { apiStatus } from "@/libs/apiError";

interface ListStateProps {
  /** A plural noun for the rows: "service accounts", "tokens". */
  what: string;
  isPending: boolean;
  error: unknown;
  isEmpty: boolean;
  onRetry: () => unknown;
  /** What to show when the list loaded and has nothing in it. */
  empty: ReactNode;
  /** Prefix for the state testids: `<testId>-loading`, `-error`, `-unavailable`. */
  testId: string;
  children: ReactNode;
}

/**
 * The four things a list can be before it is a list. Each is calm on purpose:
 * this section ships ahead of parts of its backend, so "the server doesn't do
 * this yet" is an expected answer and is shown as a fact, not as a failure.
 */
export function ListState({
  what,
  isPending,
  error,
  isEmpty,
  onRetry,
  empty,
  testId,
  children
}: ListStateProps) {
  if (isPending) {
    return (
      <div className='flex flex-col gap-2' data-testid={`${testId}-loading`}>
        <span className='sr-only'>Loading {what}</span>
        <Skeleton className='h-9 w-full' />
        <Skeleton className='h-9 w-full' />
        <Skeleton className='h-9 w-2/3' />
      </div>
    );
  }

  if (error) {
    const status = apiStatus(error);
    if (status === 404) {
      return (
        <Notice
          testId={`${testId}-unavailable`}
          title={`${capitalize(what)} aren't available here yet`}
        >
          This server doesn't support them yet. Nothing is wrong on your side, and nothing was lost.
        </Notice>
      );
    }
    if (status === 403) {
      return (
        <Notice testId={`${testId}-forbidden`} title={`You can't see ${what} here`}>
          Only an organization owner or admin can. Ask one of them for access.
        </Notice>
      );
    }
    return (
      <div
        className='flex flex-col items-start gap-2 rounded-lg border border-destructive/30 p-4 text-xs'
        data-testid={`${testId}-error`}
      >
        <p className='font-medium'>Couldn't load {what}</p>
        <p className='text-muted-foreground'>The request failed. Try again in a moment.</p>
        <Button variant='outline' size='sm' className='h-7 px-2 text-xs' onClick={() => onRetry()}>
          Try again
        </Button>
      </div>
    );
  }

  if (isEmpty) return <>{empty}</>;
  return <>{children}</>;
}

const capitalize = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);

function Notice({
  title,
  testId,
  children
}: {
  title: string;
  testId: string;
  children: ReactNode;
}) {
  return (
    <div className='flex gap-2 rounded-lg border bg-muted/40 p-4 text-xs' data-testid={testId}>
      <Info className='mt-0.5 size-3.5 shrink-0 text-muted-foreground' aria-hidden />
      <div className='flex flex-col gap-1'>
        <p className='font-medium'>{title}</p>
        <p className='text-muted-foreground leading-relaxed'>{children}</p>
      </div>
    </div>
  );
}

interface EmptyStateProps {
  icon: LucideIcon;
  title: string;
  children: ReactNode;
  /** The one thing to do next, when there is one. */
  action?: ReactNode;
  testId?: string;
}

/** An empty list is an invitation: what belongs here, and the button that starts it. */
export function EmptyState({ icon: Icon, title, children, action, testId }: EmptyStateProps) {
  return (
    <div
      className='flex flex-col items-center gap-2 rounded-lg border border-dashed px-6 py-10 text-center'
      data-testid={testId}
    >
      <Icon className='size-5 text-muted-foreground' aria-hidden />
      <p className='font-medium text-sm'>{title}</p>
      <p className='max-w-sm text-muted-foreground text-xs leading-relaxed'>{children}</p>
      {action && <div className='mt-2'>{action}</div>}
    </div>
  );
}
