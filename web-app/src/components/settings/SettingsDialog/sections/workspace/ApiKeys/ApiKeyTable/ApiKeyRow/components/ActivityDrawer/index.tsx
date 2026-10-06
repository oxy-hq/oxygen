import { Info } from "lucide-react";
import type React from "react";
import { Button } from "@/components/ui/shadcn/button";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle
} from "@/components/ui/shadcn/sheet";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import { type TokenNoun, tokenNoun } from "@/hooks/api/apiKeys/extendMessages";
import type { TokenActivityEndpoints } from "@/hooks/api/apiKeys/tokenEndpoints";
import useApiKeyActivity, { API_KEY_ACTIVITY_LIMIT } from "@/hooks/api/apiKeys/useApiKeyActivity";
import { apiStatus } from "@/libs/apiError";
import type { ApiKeyActivityResponse } from "@/types/apiKey";
import type { TokenSummary } from "@/types/apiToken";
import { splitEvents } from "./activity";
import KeyActions from "./components/KeyActions";
import KeyHistory from "./components/KeyHistory";
import LastUsedSection from "./components/LastUsedSection";
import UsageSection from "./components/UsageSection";

interface Props {
  token: TokenSummary;
  /** Where this token's activity is read from. */
  endpoints: TokenActivityEndpoints;
  /** Says what the events are limited to, when they are (an org's inventory shows only its own). */
  scopeNote?: string;
  /**
   * Set when the server withholds request counts (a person's token in an org's inventory): the
   * note replaces the chart, which would otherwise read the empty `usage` as "no requests".
   */
  usageNote?: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

const LoadingState: React.FC = () => (
  <div className='flex flex-col gap-5' data-testid='api-key-activity-loading'>
    <Skeleton className='h-16 w-full' />
    <Skeleton className='h-20 w-full' />
    <Skeleton className='h-32 w-full' />
  </div>
);

/** A 404: the server predates this feature, or the row was deleted. Neither is an error to fix here. */
const UnavailableState: React.FC<{ noun: TokenNoun }> = ({ noun }) => (
  <div
    className='flex gap-2 rounded-md border bg-muted/40 p-3 text-xs'
    data-testid='api-key-activity-unavailable'
  >
    <Info className='mt-0.5 size-3.5 shrink-0 text-muted-foreground' aria-hidden />
    <div className='flex flex-col gap-1'>
      <p className='font-medium'>Activity isn't available for this {noun}</p>
      <p className='text-muted-foreground'>
        This server doesn't record its activity yet, or the {noun} has been deleted.
      </p>
    </div>
  </div>
);

const ErrorState: React.FC<{ onRetry: () => void }> = ({ onRetry }) => (
  <div
    className='flex flex-col items-start gap-2 rounded-md border border-destructive/30 p-3 text-xs'
    data-testid='api-key-activity-error'
  >
    <p className='font-medium'>Couldn't load activity</p>
    <p className='text-muted-foreground'>The request failed. Try again in a moment.</p>
    <Button
      variant='outline'
      size='sm'
      className='h-7 px-2 text-xs'
      onClick={onRetry}
      data-testid='api-key-activity-retry'
    >
      Try again
    </Button>
  </div>
);

const UsageWithheld: React.FC<{ note: string }> = ({ note }) => (
  <section className='flex flex-col gap-2' data-testid='api-key-activity-usage-withheld'>
    <h3 className='font-medium text-xs'>Requests, last 30 days</h3>
    <p className='text-muted-foreground text-xs'>{note}</p>
  </section>
);

interface ActivityBodyProps {
  data: ApiKeyActivityResponse;
  noun: TokenNoun;
  usageNote?: string;
}

const ActivityBody: React.FC<ActivityBodyProps> = ({ data, noun, usageNote }) => {
  const { lifecycle, actions } = splitEvents(data.events);
  return (
    <>
      <LastUsedSection lastUsed={data.last_used} />
      {usageNote ? <UsageWithheld note={usageNote} /> : <UsageSection usage={data.usage ?? []} />}
      <KeyHistory events={lifecycle} noun={noun} />
      <KeyActions
        events={actions}
        truncated={data.events.length >= API_KEY_ACTIVITY_LIMIT}
        limit={API_KEY_ACTIVITY_LIMIT}
        noun={noun}
      />
    </>
  );
};

/**
 * A side sheet with one legacy API key's or one token's last use, 30-day requests, history and
 * audited actions. Which of the two it is comes from the row's `kind`, and the copy follows it.
 */
const ActivityDrawer: React.FC<Props> = ({
  token,
  endpoints,
  scopeNote,
  usageNote,
  open,
  onOpenChange
}) => {
  const { data, isLoading, error, refetch } = useApiKeyActivity(endpoints, token.id, open);
  const noun = tokenNoun(token);

  const body = () => {
    if (isLoading) return <LoadingState />;
    if (error) {
      return apiStatus(error) === 404 ? (
        <UnavailableState noun={noun} />
      ) : (
        <ErrorState onRetry={() => refetch()} />
      );
    }
    return data ? <ActivityBody data={data} noun={noun} usageNote={usageNote} /> : null;
  };

  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent className='w-full gap-0 sm:max-w-md' data-testid='api-key-activity-drawer'>
        <SheetHeader className='border-b'>
          <SheetTitle className='truncate pr-6 text-sm'>{token.name}</SheetTitle>
          <SheetDescription className='text-xs'>
            Activity
            {token.masked_key && <span className='ml-2 font-mono'>{token.masked_key}</span>}
          </SheetDescription>
        </SheetHeader>
        <div className='flex flex-1 flex-col gap-6 overflow-y-auto p-4'>
          {scopeNote && (
            <p className='text-muted-foreground text-xs' data-testid='api-key-activity-scope'>
              {scopeNote}
            </p>
          )}
          {body()}
        </div>
      </SheetContent>
    </Sheet>
  );
};

export default ActivityDrawer;
