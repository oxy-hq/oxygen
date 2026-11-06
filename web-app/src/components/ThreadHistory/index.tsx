import { Search, X } from "lucide-react";
import { useEffect, useState } from "react";
import { ThreadRow } from "@/components/ThreadRow";
import { Input } from "@/components/ui/shadcn/input";
import { Spinner } from "@/components/ui/shadcn/spinner";
import useThreads from "@/hooks/api/threads/useThreads";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { cn } from "@/libs/shadcn/utils";
import ROUTES from "@/libs/utils/routes";
import useCurrentOrg from "@/stores/useCurrentOrg";

const DEFAULT_INITIAL = 8;
const STEP = 8;
// The backend clamps `limit` to 100, so paging can't grow past it — beyond
// this, browse caps out and search is the way to reach older threads.
const MAX = 100;

interface ThreadHistoryProps {
  className?: string;
  /** Number of threads shown before "Show more" (and the page size). */
  initial?: number;
  /** When set, rows are buttons calling this (load the thread in place);
   *  otherwise they are router links to the thread page. */
  onSelect?: (threadId: string) => void;
  /** Show the "Recent threads" label. Off in the Ask dock, where the dock
   *  header already labels the view. */
  showLabel?: boolean;
}

/**
 * A searchable, paginated thread list. Search is server-side (case-insensitive
 * over title + input, debounced); "Show more" grows the page up to the
 * backend's 100-row cap. Shared by the chat page (navigates on click) and the
 * Ask dock history view (loads the thread in place via `onSelect`).
 */
export function ThreadHistory({
  className,
  initial = DEFAULT_INITIAL,
  onSelect,
  showLabel = true
}: ThreadHistoryProps) {
  const { project } = useCurrentProjectBranch();
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";
  const ws = ROUTES.ORG(orgSlug).WORKSPACE(project.id);

  const [query, setQuery] = useState("");
  const [search, setSearch] = useState("");
  const [limit, setLimit] = useState(initial);

  // Debounce the input so we don't query the server on every keystroke.
  useEffect(() => {
    const t = setTimeout(() => setSearch(query.trim()), 250);
    return () => clearTimeout(t);
  }, [query]);
  // A new search resets paging back to the first page. Depends on `search` to
  // re-run on change even though the body reads only `initial`.
  // biome-ignore lint/correctness/useExhaustiveDependencies: reset on search change
  useEffect(() => {
    setLimit(initial);
  }, [search, initial]);

  const { data, isLoading } = useThreads({ limit, search: search || undefined });
  const threads = data?.threads ?? [];
  const total = data?.pagination.total ?? 0;
  const hasMore = data?.pagination.has_next ?? false;
  const isSearching = search.length > 0;

  // Hide the whole section on a fresh, thread-less workspace (no active search).
  if (!isSearching && !isLoading && total === 0) return null;

  return (
    <div className={cn("flex flex-col gap-2", className)}>
      {showLabel && (
        <p className='font-medium text-muted-foreground/70 text-xs uppercase tracking-wide'>
          Recent threads
        </p>
      )}

      <div className='relative'>
        <Search className='absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-muted-foreground' />
        <Input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder='Search threads…'
          aria-label='Search threads'
          data-testid='thread-history-search'
          className='h-9 pr-8 pl-8 text-base md:text-sm'
        />
        {query && (
          <button
            type='button'
            onClick={() => setQuery("")}
            aria-label='Clear search'
            className='absolute top-1/2 right-2 -translate-y-1/2 text-muted-foreground hover:text-foreground'
          >
            <X className='size-3.5' />
          </button>
        )}
      </div>

      {isLoading ? (
        <div className='flex justify-center py-4'>
          <Spinner className='size-4' />
        </div>
      ) : threads.length === 0 ? (
        isSearching ? (
          <p className='px-1 py-2 text-muted-foreground/70 text-xs'>No threads match “{search}”.</p>
        ) : null
      ) : (
        <div data-testid='thread-history-list'>
          {threads.map((t) => (
            <ThreadRow
              key={t.id}
              title={t.title || t.input || "(untitled thread)"}
              timestamp={t.created_at}
              to={ws.THREAD(t.id)}
              onSelect={onSelect ? () => onSelect(t.id) : undefined}
            />
          ))}
        </div>
      )}

      {hasMore && threads.length < MAX && (
        <button
          type='button'
          onClick={() => setLimit((l) => Math.min(l + STEP, MAX))}
          data-testid='thread-history-show-more'
          className='mt-0.5 w-full rounded px-1 py-1.5 text-center font-medium text-muted-foreground/70 text-xs uppercase tracking-wide hover:text-foreground'
        >
          Show more
        </button>
      )}
    </div>
  );
}
