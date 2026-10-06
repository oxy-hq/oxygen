import type React from "react";
import type { TokenNoun } from "@/hooks/api/apiKeys/extendMessages";
import { cn } from "@/libs/shadcn/utils";
import type { ApiKeyActivityEvent } from "@/types/apiKey";
import { RelativeTime } from "./RelativeTime";

const ActionItem: React.FC<{ event: ApiKeyActivityEvent }> = ({ event }) => {
  const failed = event.outcome !== "success";
  return (
    <li
      className={cn("flex flex-col gap-0.5 py-1.5", failed && "bg-destructive/5")}
      title={event.reason ?? undefined}
      data-testid='api-key-activity-action-item'
    >
      <div className='flex items-baseline gap-2'>
        <span className='truncate font-mono'>{event.action}</span>
        {failed && <span className='shrink-0 font-medium text-destructive'>failed</span>}
        <RelativeTime iso={event.created_at} className='ml-auto shrink-0 text-muted-foreground' />
      </div>
      {(event.target_label || event.target_type) && (
        <span className='truncate text-muted-foreground' title={event.target_id ?? undefined}>
          {event.target_label || event.target_type}
        </span>
      )}
    </li>
  );
};

interface Props {
  events: ApiKeyActivityEvent[];
  /** The server returned as many events as asked for, so older ones exist. */
  truncated: boolean;
  limit: number;
  /** "legacy API key" or "token": the drawer is shared, and the two are never swapped. */
  noun: TokenNoun;
}

/** What the key or token was used to do, as recorded in the audit log. Newest first. */
const KeyActions: React.FC<Props> = ({ events, truncated, limit, noun }) => (
  <section className='flex flex-col gap-1' data-testid='api-key-activity-actions'>
    <h3 className='font-medium text-xs'>Actions with this {noun}</h3>
    {events.length === 0 ? (
      <p className='text-muted-foreground text-xs'>
        No audited actions yet. Every request made with it still counts in the chart above.
      </p>
    ) : (
      <ol className='flex flex-col divide-y divide-border/60 text-xs'>
        {events.map((e) => (
          <ActionItem key={e.id} event={e} />
        ))}
      </ol>
    )}
    {truncated && (
      <p className='text-muted-foreground text-xs'>Showing the latest {limit} events.</p>
    )}
  </section>
);

export default KeyActions;
