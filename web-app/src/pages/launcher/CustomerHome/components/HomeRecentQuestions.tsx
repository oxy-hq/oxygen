import { Link } from "react-router-dom";
import useThreads from "@/hooks/api/threads/useThreads";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { timeAgo } from "@/libs/utils/date";
import ROUTES from "@/libs/utils/routes";
import useCurrentOrg from "@/stores/useCurrentOrg";

const RECENT_LIMIT = 3;

export type RecentQuestion = { id: string; title: string; timestamp: string };

/** The person's three latest questions, newest first, and whether the answer is in
 *  (`settled`, also on an error). The home reads it to know whether the side column
 *  has anything to hold; the list draws the rows. */
export function useRecentQuestions(): { rows: RecentQuestion[]; settled: boolean } {
  // `isLoading`, not `isPending`: a query that never runs is settled, not waiting.
  const { data, isLoading } = useThreads({ page: 1, limit: RECENT_LIMIT });
  const rows = (data?.threads ?? [])
    .map((t) => ({
      id: t.id,
      title: t.title || t.input || "Untitled question",
      timestamp: t.created_at
    }))
    .sort((a, b) => new Date(b.timestamp).getTime() - new Date(a.timestamp).getTime())
    .slice(0, RECENT_LIMIT);
  return { rows, settled: !isLoading };
}

/**
 * The person's latest questions, one line each: a shortcut back into a thread,
 * not a section of its own. It takes no room it has not earned: nothing while
 * it loads, and nothing when there are no questions yet.
 */
export function HomeRecentQuestions({ rows }: { rows: RecentQuestion[] }) {
  const { project } = useCurrentProjectBranch();
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";
  const ws = ROUTES.ORG(orgSlug).WORKSPACE(project.id);
  if (rows.length === 0) return null;

  return (
    <section
      className='flex flex-col gap-1'
      aria-labelledby='home-recent-heading'
      data-testid='home-recent-questions'
    >
      <h2 id='home-recent-heading' className='font-medium text-muted-foreground text-sm'>
        Recent questions
      </h2>
      <ul className='flex flex-col'>
        {rows.map((row) => (
          <li key={row.id}>
            <Link
              to={ws.THREAD(row.id)}
              data-testid='home-recent-question'
              className='-mx-2 flex min-h-11 items-center gap-3 rounded-md px-2 text-sm transition-colors duration-(--duration-fast) hover:bg-accent focus-visible:bg-accent focus-visible:outline-none'
            >
              <span className='min-w-0 flex-1 truncate text-foreground'>{row.title}</span>
              <span className='shrink-0 text-muted-foreground text-xs'>
                {timeAgo(row.timestamp)}
              </span>
            </Link>
          </li>
        ))}
      </ul>
    </section>
  );
}
