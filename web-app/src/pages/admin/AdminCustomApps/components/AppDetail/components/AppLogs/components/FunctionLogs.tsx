import { useState } from "react";
import { Input } from "@/components/ui/shadcn/input";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import { useAppLogs } from "@/hooks/api/customApps/useCustomApps";
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
import { InvocationLogs } from "./InvocationLogs";

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
        <p className='text-muted-foreground text-xs' data-testid='admin-app-logs-error'>
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

  return (
    <div className='space-y-1.5'>
      <div
        className='max-h-80 overflow-auto rounded-md border bg-muted/20'
        data-testid='admin-app-logs-list'
      >
        {groupByInvocation(lines).map((group) => (
          <InvocationLogs key={group.key} group={group} withDate={hours > 24} />
        ))}
      </div>
      {/* A full page is a cut, not the whole window. Without saying so, an
          invocation that straddles the cut reads as one that printed only its
          last few lines, and "nothing before 14:02" reads as "nothing happened". */}
      {lines.length >= limit && (
        <p className='text-muted-foreground text-xs' data-testid='admin-app-logs-truncated'>
          Showing the newest {limit.toLocaleString()} lines of {windowPhrase(hours)}. Older output
          is not shown, so an invocation that began before them appears without its first lines.
          {/* Not "narrow the window": the route returns the newest lines, so a
              shorter window is a subset of the same recent stretch. */}
          {!byRequest && " Filter by request id to reach a specific invocation."}
        </p>
      )}
    </div>
  );
};
