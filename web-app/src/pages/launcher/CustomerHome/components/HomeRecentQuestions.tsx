import { Link } from "react-router-dom";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import useThreads from "@/hooks/api/threads/useThreads";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { cn } from "@/libs/shadcn/utils";
import { timeAgo } from "@/libs/utils/date";
import ROUTES from "@/libs/utils/routes";
import useCurrentOrg from "@/stores/useCurrentOrg";
import { HomePanel } from "./HomePanel";

const RECENT_LIMIT = 5;

/**
 * "Your recent questions": the person's five latest threads, newest first, in a
 * panel the same height as the world model card beside it. Each row is the
 * question and when it was asked; the whole row opens the thread.
 */
export function HomeRecentQuestions({
  canAsk,
  className
}: {
  /** Whether chat is usable now; only changes what the empty panel says. */
  canAsk: boolean;
  className?: string;
}) {
  const { project } = useCurrentProjectBranch();
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";
  const ws = ROUTES.ORG(orgSlug).WORKSPACE(project.id);
  const { data, isLoading } = useThreads({ page: 1, limit: RECENT_LIMIT });

  const rows = (data?.threads ?? [])
    .map((t) => ({
      id: t.id,
      title: t.title || t.input || "Untitled question",
      timestamp: t.created_at
    }))
    .sort((a, b) => new Date(b.timestamp).getTime() - new Date(a.timestamp).getTime())
    .slice(0, RECENT_LIMIT);

  return (
    <HomePanel title='Your recent questions' className={className} testId='home-recent-questions'>
      {isLoading ? (
        <div className='flex flex-col gap-2 px-4 py-3'>
          <Skeleton className='h-5 w-full' />
          <Skeleton className='h-5 w-4/5' />
          <Skeleton className='h-5 w-3/5' />
        </div>
      ) : rows.length === 0 ? (
        <p className='px-4 py-3 text-muted-foreground text-sm'>
          {canAsk
            ? "Questions you ask show up here."
            : "Questions you ask will show up here once chat is ready."}
        </p>
      ) : (
        <ul className='flex flex-col py-1' data-testid='recent-threads-list'>
          {rows.map((row, i) => (
            <li key={row.id}>
              <Link
                to={ws.THREAD(row.id)}
                data-testid='thread-row'
                className={cn(
                  "flex flex-col gap-1 px-4 py-2 transition-colors hover:bg-accent focus-visible:bg-accent focus-visible:outline-none",
                  i > 0 && "border-t"
                )}
              >
                <span className='line-clamp-2 text-foreground text-sm'>{row.title}</span>
                <span className='text-muted-foreground text-xs'>{timeAgo(row.timestamp)}</span>
              </Link>
            </li>
          ))}
        </ul>
      )}
    </HomePanel>
  );
}
