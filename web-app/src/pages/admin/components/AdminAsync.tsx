import { RotateCw, TriangleAlert } from "lucide-react";
import type { ReactNode } from "react";
import { Button } from "@/components/ui/shadcn/button";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import { cn } from "@/libs/shadcn/utils";

/**
 * The three states every fetched thing on this surface can be in — loading, failed,
 * empty — stated once.
 *
 * Fifteen places spelled the failure out by hand, each with its own wrapper classes and
 * its own sentence ("Failed to load compiles." / "Failed to load queue stats." /
 * "Failed to load the audit log."), and **not one of them offered a retry** — the
 * operator's only recourse was a full page reload, on a console whose whole job is
 * triage. Some rendered one skeleton bar, some three, some a spinner, some nothing.
 *
 * The call site now names the noun and hands over the render:
 *
 * ```tsx
 * <AdminAsync query={health} noun='workspace health' rows={4}>
 *   {(data) => <HealthTable workspaces={data.workspaces} />}
 * </AdminAsync>
 * ```
 *
 * `query` is structurally typed, not tied to React Query, so a hook that composes two
 * queries can pass `{ isPending, isError, data, refetch }` of its own.
 */
export type AdminAsyncQuery<T> = {
  isPending?: boolean;
  isLoading?: boolean;
  isError?: boolean;
  data: T | undefined;
  refetch?: () => unknown;
  /**
   * What the server said. Rendered under the headline when it is a real message, because
   * "Couldn't load airhouse warehouses" does not tell an operator whether to retry, fix a
   * credential, or page someone — and some pages surfaced that detail before this
   * component existed.
   */
  error?: unknown;
};

/** The server's own words, when there are any worth showing. */
function errorDetail(error: unknown): string | null {
  const raw =
    error instanceof Error
      ? error.message
      : typeof error === "string"
        ? error
        : typeof error === "object" && error !== null && "message" in error
          ? String((error as { message: unknown }).message)
          : null;
  const trimmed = raw?.trim();
  if (!trimmed) return null;
  // Axios's default is "Request failed with status code 500" — the status is already
  // implied by the failure, and the string reads like a stack trace leaked into the UI.
  if (/^request failed with status code \d+$/i.test(trimmed)) return null;
  return trimmed.length > 300 ? `${trimmed.slice(0, 300)}…` : trimmed;
}

export function AdminAsync<T>({
  query,
  noun,
  rows = 3,
  skeleton,
  isEmpty,
  empty,
  className,
  children
}: {
  query: AdminAsyncQuery<T>;
  /**
   * What failed to load, as it reads mid-sentence: `noun='the audit log'` →
   * "Couldn't load the audit log." Lowercase, no trailing period.
   */
  noun: string;
  /** Skeleton bars to show while loading. Match the real content's row count. */
  rows?: number;
  /** A bespoke loading shape, when bars would misrepresent the content. */
  skeleton?: ReactNode;
  /** Say when loaded data counts as empty — usually `(d) => d.items.length === 0`. */
  isEmpty?: (data: T) => boolean;
  /** What to show then. Usually an `<AdminEmptyState>`. */
  empty?: ReactNode;
  className?: string;
  children: (data: T) => ReactNode;
}) {
  const loading = query.isPending ?? query.isLoading ?? false;

  if (loading) {
    return (
      <div className={className} data-testid='admin-async-loading'>
        {skeleton ?? (
          <div className='space-y-2'>
            {Array.from({ length: rows }, (_, i) => (
              // biome-ignore lint/suspicious/noArrayIndexKey: static skeleton placeholders
              <Skeleton key={i} className='h-10 w-full' />
            ))}
          </div>
        )}
      </div>
    );
  }

  // `!data` counts as failure: a query that resolved to nothing cannot be rendered, and
  // every call site used to repeat `isError || !data` — or forget the second half and
  // crash on `data.workspaces`.
  if (query.isError || query.data === undefined) {
    const detail = errorDetail(query.error);
    return (
      <div
        data-testid='admin-async-error'
        className={cn(
          "rounded-lg border border-destructive/40 bg-destructive/5 p-4 text-status-error-text text-xs",
          className
        )}
      >
        <div className='flex flex-wrap items-center gap-x-3 gap-y-2'>
          <TriangleAlert className='size-3.5 shrink-0' />
          <span>Couldn&rsquo;t load {noun}.</span>
          {query.refetch ? (
            <Button
              variant='outline'
              size='sm'
              className='ml-auto h-6 gap-1.5 px-2 text-xs'
              onClick={() => query.refetch?.()}
              data-testid='admin-async-retry'
            >
              <RotateCw className='size-3' />
              Retry
            </Button>
          ) : null}
        </div>
        {detail ? (
          <p
            data-testid='admin-async-error-detail'
            className='mt-2 whitespace-pre-wrap break-words pl-6 font-mono text-[11px] opacity-80'
          >
            {detail}
          </p>
        ) : null}
      </div>
    );
  }

  if (isEmpty?.(query.data) && empty) {
    return (
      <div className={className} data-testid='admin-async-empty'>
        {empty}
      </div>
    );
  }

  return <>{children(query.data)}</>;
}
