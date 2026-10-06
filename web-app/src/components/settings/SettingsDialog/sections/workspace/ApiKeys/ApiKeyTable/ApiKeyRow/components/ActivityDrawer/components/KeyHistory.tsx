import { Ban, CalendarPlus, KeyRound, type LucideIcon, RefreshCw } from "lucide-react";
import type React from "react";
import type { TokenNoun } from "@/hooks/api/apiKeys/extendMessages";
import { cn } from "@/libs/shadcn/utils";
import { ApiKeyService } from "@/services/api/apiKey";
import type { ApiKeyActivityEvent } from "@/types/apiKey";
import { type ExpiryChange, expiryChange, lifecycleLabel } from "../activity";
import { RelativeTime } from "./RelativeTime";

const ICONS: Record<string, LucideIcon> = {
  "token.created": KeyRound,
  "token.extended": CalendarPlus,
  "token.regenerated": RefreshCw,
  "token.revoked": Ban
};

const expiryText = (iso: string | null) => (iso ? ApiKeyService.formatDay(iso) : "no expiry");

/** "Oct 3, 2026 → Jan 1, 2027": the old expiry recedes, the new one is what holds now. */
const ExpiryDelta: React.FC<{ change: ExpiryChange }> = ({ change }) => (
  <span className='tabular-nums' data-testid='api-key-activity-expiry-change'>
    <span className='text-muted-foreground line-through'>{expiryText(change.from)}</span>
    <span className='px-1 text-muted-foreground' aria-hidden>
      →
    </span>
    <span className='sr-only'>changed to</span>
    <span className='font-medium'>{expiryText(change.to)}</span>
  </span>
);

const HistoryItem: React.FC<{ event: ApiKeyActivityEvent }> = ({ event }) => {
  const Icon = ICONS[event.action] ?? KeyRound;
  const change = event.action === "token.extended" ? expiryChange(event) : null;
  const failed = event.outcome !== "success";
  return (
    <li
      className='flex gap-2 py-1.5'
      data-testid='api-key-activity-history-item'
      data-action={event.action}
    >
      <Icon
        className={cn(
          "mt-0.5 size-3.5 shrink-0",
          event.action === "token.revoked" ? "text-destructive" : "text-muted-foreground"
        )}
        aria-hidden
      />
      <div className='flex min-w-0 flex-1 flex-col gap-0.5'>
        <div className='flex items-baseline gap-2'>
          <span className='font-medium'>{lifecycleLabel(event.action)}</span>
          {failed && <span className='font-medium text-destructive'>failed</span>}
          <RelativeTime iso={event.created_at} className='ml-auto shrink-0 text-muted-foreground' />
        </div>
        {change && <ExpiryDelta change={change} />}
        <span className='truncate text-muted-foreground' title={event.reason ?? undefined}>
          by {event.actor_email}
        </span>
      </div>
    </li>
  );
};

interface Props {
  events: ApiKeyActivityEvent[];
  /** "legacy API key" or "token": the drawer is shared, and the two are never swapped. */
  noun: TokenNoun;
}

/** What was done to the key or token: created, extended, revoked. Newest first. */
const KeyHistory: React.FC<Props> = ({ events, noun }) => (
  <section className='flex flex-col gap-1' data-testid='api-key-activity-history'>
    <h3 className='font-medium text-xs'>History</h3>
    {events.length === 0 ? (
      <p className='text-muted-foreground text-xs'>No changes recorded for this {noun} yet.</p>
    ) : (
      <ol className='flex flex-col divide-y divide-border/60 text-xs'>
        {events.map((e) => (
          <HistoryItem key={e.id} event={e} />
        ))}
      </ol>
    )}
  </section>
);

export default KeyHistory;
