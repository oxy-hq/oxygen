import { Info, TriangleAlert } from "lucide-react";
import { useState } from "react";
import { Input } from "@/components/ui/shadcn/input";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import { useAppLogs } from "@/hooks/api/customApps/useCustomApps";
import { cn } from "@/libs/shadcn/utils";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { FunctionLogLine } from "@/types/apps";
import {
  groupByInvocation,
  type LogWindowHours,
  parseRequestFilter,
  REQUEST_LOG_LIMIT,
  WIDEST_LOG_WINDOW,
  WINDOW_LOG_LIMIT,
  windowPhrase
} from "../functionLogs";
import { InvocationLogs, LOG_TRACKS } from "./InvocationLogs";

/**
 * Function output, one invocation at a time, optionally narrowed to a request.
 *
 * A request id is one needle, so it is looked for across everything the route
 * will search rather than only the window the list happens to be on — a ticket
 * about Tuesday should not come back empty because the picker says 24h.
 */
export const FunctionLogs = ({
  orgSlug,
  appSlug,
  hours
}: {
  orgSlug: string;
  appSlug: string;
  hours: LogWindowHours;
}) => {
  const [requestInput, setRequestInput] = useState("");
  const filter = parseRequestFilter(requestInput);
  const query =
    filter.kind === "id"
      ? { hours: WIDEST_LOG_WINDOW, limit: REQUEST_LOG_LIMIT, requestId: filter.requestId }
      : { hours, limit: WINDOW_LOG_LIMIT };
  const { data, isLoading, error } = useAppLogs(orgSlug, appSlug, query);

  return (
    <div className='space-y-2'>
      <Input
        value={requestInput}
        onChange={(e) => setRequestInput(e.target.value)}
        placeholder='Filter by request id (x-oxy-request-id)'
        aria-label='Filter function output by request id'
        className='h-8 font-mono text-xs'
        data-testid='admin-app-logs-request-filter'
      />
      {filter.kind === "invalid" ? (
        <p className='text-muted-foreground text-xs' data-testid='admin-app-logs-filter-invalid'>
          A request id is a UUID — the <code>x-oxy-request-id</code> header on the app&rsquo;s
          response.
        </p>
      ) : isLoading ? (
        <Skeleton className='h-24 w-full' />
      ) : error ? (
        // A failed read is a failure, and is drawn as one: in grey it was the
        // same paragraph as "nothing was printed".
        <p
          className={cn("flex items-center gap-1.5 text-xs", ADMIN_TONE.danger.text)}
          data-testid='admin-app-logs-error'
        >
          <TriangleAlert className='size-3 shrink-0' aria-hidden />
          Could not read function logs.
        </p>
      ) : (
        <FunctionLogList
          lines={data ?? []}
          limit={query.limit}
          hours={query.hours}
          byRequest={filter.kind === "id"}
        />
      )}
    </div>
  );
};

const FunctionLogList = ({
  lines,
  limit,
  hours,
  byRequest
}: {
  lines: FunctionLogLine[];
  limit: number;
  hours: LogWindowHours;
  byRequest: boolean;
}) => {
  if (lines.length === 0) {
    return (
      <p className='text-muted-foreground text-xs' data-testid='admin-app-logs-empty'>
        {byRequest
          ? `No function output for this request in ${windowPhrase(hours)}.`
          : `No function output in ${windowPhrase(hours)}. An app with no Oxy Functions, and one whose functions printed nothing, both look like this.`}
      </p>
    );
  }

  const withDate = hours > 24;
  return (
    <div className='overflow-hidden rounded-md border'>
      <div className='flex gap-3 border-border/60 border-b px-3 py-1.5 text-[10px] text-muted-foreground uppercase tracking-[0.16em]'>
        <span className={cn("shrink-0", LOG_TRACKS.time(withDate))}>UTC</span>
        <span className={cn("shrink-0", LOG_TRACKS.level)}>Level</span>
        <span>Output</span>
      </div>
      <div className='max-h-80 overflow-auto' data-testid='admin-app-logs-list'>
        {groupByInvocation(lines).map((group) => (
          <InvocationLogs key={group.key} group={group} withDate={withDate} />
        ))}
      </div>
      {/* A full page is a cut, not the whole window. Without saying so, an
          invocation that straddles the cut reads as one that printed only its
          last few lines, and "nothing before 14:02" reads as "nothing happened".
          Said as the foot of the list it cuts, inside the same frame. */}
      {lines.length >= limit && (
        <p
          className='flex items-start gap-1.5 border-border/60 border-t bg-muted/40 px-3 py-2 text-xs'
          data-testid='admin-app-logs-truncated'
        >
          <Info className='mt-0.5 size-3 shrink-0 text-muted-foreground' aria-hidden />
          <span>
            Showing the newest{" "}
            <span className='font-medium tabular-nums'>{limit.toLocaleString()}</span> lines of{" "}
            {windowPhrase(hours)}. Older output is not shown, so an invocation that began before
            them appears without its first lines.
            {/* Not "narrow the window": the route returns the newest lines, so a
                shorter window is a subset of the same recent stretch. */}
            {!byRequest && " Filter by request id to reach a specific invocation."}
          </span>
        </p>
      )}
    </div>
  );
};
