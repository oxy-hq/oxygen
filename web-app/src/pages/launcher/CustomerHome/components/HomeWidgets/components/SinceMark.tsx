import type { Since } from "../../../useSinceLastVisit";

const LABEL: Record<Since, string> = { new: "New", updated: "Updated" };

/** "New" or "Updated" beside a widget that changed while the person was away. */
export function SinceMark({ since, testId }: { since: Since; testId: string }) {
  return (
    <span
      data-testid={testId}
      data-since={since}
      className='shrink-0 rounded-full bg-primary/10 px-1.5 font-medium text-primary text-xs'
    >
      {LABEL[since]}
    </span>
  );
}
