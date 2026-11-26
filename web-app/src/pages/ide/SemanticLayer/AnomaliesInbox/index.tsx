import { AlertTriangle, RefreshCw, X } from "lucide-react";
import { useMemo, useState } from "react";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import { Badge } from "@/components/ui/shadcn/badge";
import { Button } from "@/components/ui/shadcn/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/shadcn/tabs";
import TablePagination from "@/components/ui/TablePagination";
import {
  useMetricAnomalies,
  useMonitors,
  useScanMetricAnomalies,
  useUpdateAnomalyStatus
} from "@/hooks/api/useMetricAnomalies";
import { cn } from "@/libs/shadcn/utils";
import type { AnomalyStatus, MetricAnomaly, ScanFailure } from "@/types/metricAnomalies";
import AnomalyTable, { formatSegment } from "./components/AnomalyTable";
import BulkActionBar from "./components/BulkActionBar";
import { canMoveTo, groupIntoEvents, targetOf } from "./components/events";
import ExplainDrawer from "./ExplainDrawer";
import MonitorsTab from "./MonitorsTab";

/** The largest `limit` the server accepts; past it the request is clamped.
 *  `PAGE_SIZE` is pinned inside it structurally rather than by comment: the
 *  offsets we request are computed from `PAGE_SIZE` while every page number is
 *  computed from the echoed `limit`, so a page size the server would clamp puts
 *  the two in different units — and the render-phase reconcile then chases a
 *  `servedPage` that grows faster than `page`, walking a page turn to the last
 *  page instead of the next one. */
const MAX_SERVER_LIMIT = 500;

/** Events per page. The server pages events and returns every bucket of each,
 *  so a page is up to `PAGE_SIZE × buckets-per-event` rows — 25 keeps that
 *  payload sane while still filling the table.
 *
 *  Exported so the tab badge can ask for the *same* first page rather than a
 *  page of its own: same query key, so the two share one cache entry and one
 *  request.
 *
 *  Must stay inside the server's `1..=500` clamp on `limit`. Requested offsets
 *  are computed from this value, so a page size the server would clamp puts the
 *  two out of step; everything derived from a *response* divides by the echoed
 *  `limit` instead, which is what makes the mismatch visible rather than
 *  silent. */
export const PAGE_SIZE = Math.min(25, MAX_SERVER_LIMIT);

/** Fallback for the deepest offset the server will serve, used only until a
 *  response tells us — every response echoes `max_offset`, so the real number
 *  comes from the server rather than from a copy here that could drift. */
const MAX_OFFSET_FALLBACK = 50_000;

/** The inbox's own first page — what the badge reuses. */
export const FIRST_PAGE = { limit: PAGE_SIZE, offset: 0 } as const;

const STATUS_OPTIONS: { value: AnomalyStatus | "all"; label: string }[] = [
  { value: "new", label: "New" },
  { value: "acknowledged", label: "Acknowledged" },
  { value: "dismissed", label: "Dismissed" },
  { value: "all", label: "All" }
];

/**
 * Insights inbox — surfaces anomalies detected by `oxy-metric-monitoring`.
 * Contains two inner sub-tabs:
 *   - Inbox: anomaly list with status filter, scan trigger, explain drawer
 *   - Monitors: read-only list of `.monitor.yml` entries
 */
export default function AnomaliesInbox() {
  const [statusFilter, setStatusFilter] = useState<AnomalyStatus | "all">("new");
  const [asOf, setAsOf] = useState<string>("");
  const [selectedAnomaly, setSelectedAnomaly] = useState<MetricAnomaly | null>(null);
  const [failuresDismissed, setFailuresDismissed] = useState(false);
  const [page, setPage] = useState(1);
  const [selectedKeys, setSelectedKeys] = useState<Set<string>>(new Set());
  // Declared before the page clamp below, which calls it during render — and
  // it keeps the empty Set it already has, so clearing twice can't schedule a
  // second render.
  const clearSelection = () =>
    setSelectedKeys((prev) => (prev.size === 0 ? prev : new Set<string>()));

  const { data, isLoading, isFetching, isPlaceholderData, error, refetch } = useMetricAnomalies(
    statusFilter === "all" ? undefined : statusFilter,
    undefined,
    page === 1 ? FIRST_PAGE : { limit: PAGE_SIZE, offset: (page - 1) * PAGE_SIZE }
  );
  // The page size the server actually served. Every page number derived below
  // divides by this rather than by `PAGE_SIZE`, because `limit` is clamped
  // server-side (1..=500) — counting in a size the server didn't page in makes
  // pages silently overlap or skip. The offset above is the one place that has
  // to use `PAGE_SIZE`, since it is what produces this response; see the
  // constant's own note on staying inside the clamp.
  //
  // `||`, not `??`: a `0` here is a divisor, and it reaches `deepestPage`,
  // `totalPages` and `servedPage` past `echoesOffset`, which guards the other
  // field. This server clamps `limit` to 1..=500 so it cannot send one — that
  // is exactly why it costs one character.
  const served = data?.limit || PAGE_SIZE;
  // The server refuses an offset past this, so it is also the last page worth
  // offering — in the pager's numbered links and in the uncounted Next alike.
  const maxOffset = data?.max_offset ?? MAX_OFFSET_FALLBACK;
  const deepestPage = Math.floor(maxOffset / served) + 1;
  // The server pages *events*, and each carries every one of its buckets, so
  // `anomalies.length` is a row count — `total` is what the pager counts.
  //
  // `undefined` is not zero. The server drops `total` rather than failing the
  // request when its count query errors, so reading a missing total as 0 would
  // turn that deliberate degradation into a break: a full page of rows under
  // "0 anomalies", the pager gone, and the user bounced to page 1 with their
  // selection wiped.
  const anomalies = data?.anomalies;
  const totalEvents = data?.total;
  const totalPages =
    totalEvents === undefined
      ? undefined
      : // Clamped to what the server will actually serve. Offering a last-page
        // link past the offset cap turns a documented refusal into a dead
        // button.
        Math.min(Math.max(1, Math.ceil(totalEvents / served)), deepestPage);
  // Which page the server actually served, read off the response rather than
  // assumed from `page` — the two can disagree whenever a request was answered
  // by something other than the page we asked for.
  //
  // Divided by the echoed `limit`, not our own: `limit` is clamped 1..=500, so
  // counting in a size the server didn't page in is what makes pages overlap
  // or skip.
  // Did this response say which page it is? Mid-deploy a replica serving the
  // pre-paging shape echoes no `offset`, ignores the one we sent, and serves
  // page 1 for every request — so nothing derived from a page number is true of
  // its rows. One term, read by everything that would otherwise have to
  // rediscover it: the page, the count, and the uncounted pager's end.
  const echoesOffset = Number.isFinite(data?.offset);
  // Falls back to our own page rather than trusting the arithmetic: NaN here
  // would reach `setPage` and then the next request as `?offset=NaN` — a 400
  // the inbox can't get out of on its own.
  const servedPage = echoesOffset ? Math.floor((data?.offset ?? 0) / served) + 1 : page;
  // Acking the last event on the last page (or a scan that resolves rows) can
  // leave the current page past the end — land on the last real page instead of
  // an empty table the user has to click out of.
  //
  // Only ever off a real response: a failed list call leaves `data` undefined
  // (placeholder data covers pending, not error) and `retry: false` means one
  // blip is enough, so clamping on `total ?? 0` would read as "1 page" and
  // silently bounce someone off page 4.
  //
  // `!isPlaceholderData` because a placeholder still describes the page we just
  // left: reconciling against its `offset` would drag every page turn straight
  // back to where it started.
  if (data && !isPlaceholderData) {
    // `totalPages ?? Infinity`: with no total there is no last page to clamp
    // to, but the served offset is still authoritative.
    const settled = Math.min(totalPages ?? Number.POSITIVE_INFINITY, servedPage);
    if (page !== settled) {
      setPage(settled);
      // Same reason `goToPage` clears: selection is page-scoped. A scan-poll
      // refetch can clamp underneath the user, and their page-2 picks would
      // land on page 1 still checked, with the bulk bar showing a count they
      // never set here.
      clearSelection();
    }
  }

  const events = useMemo(
    () => groupIntoEvents(anomalies ?? [], data?.truncated_events),
    [anomalies, data?.truncated_events]
  );
  // What to show as the count. `total` when the server could give one;
  // otherwise everything up to and including this page, which is a floor.
  //
  // Both halves come from the same response, and both are load-bearing.
  //
  // `servedPage`, not `page`: during a page turn the placeholder frame holds
  // page 3's rows while `page` already says 4, and multiplying those overstates
  // the floor by a page — "100+" flashing before it settles on "75+".
  //
  // And `undefined` rather than a number when there is no response at all: a
  // failed fetch leaves `data` undefined and `events` empty (`retry: false`,
  // and placeholder data does not cover the error state), so the arithmetic
  // would print an exact-looking "50 anomalies" over an error banner with no
  // rows beneath it. Nothing is the honest render there.
  //
  // The pages-behind term needs `echoesOffset` too: a pre-paging replica serves
  // page 1's rows whatever we asked for, so crediting the pages we think we are
  // past would report "50+" over the first 25. With no page to stand on, this
  // page's own rows are the whole floor.
  const pagesBehind = echoesOffset ? (servedPage - 1) * served : 0;
  const countFloor = totalEvents ?? (data ? pagesBehind + events.length : undefined);
  // A full page is the only evidence of a next one when there is no total.
  // Stepping onto an empty page is recoverable — Previous is right there — and
  // that is the whole of the degraded-path pager: no remembered end, no
  // walk-back, no bounce.
  const isFullPage = events.length >= served;

  // Intersect rather than trusting the raw key set: an acked event leaves the
  // "New" page under it, and a stale key left selected would inflate the count
  // and re-send ids that are no longer on screen.
  const selectedEvents = useMemo(
    () => events.filter((e) => selectedKeys.has(e.key)),
    [events, selectedKeys]
  );

  // One mutation for every status write on this screen — row buttons and the
  // batch bar alike. Two instances would each see only their own `isPending`,
  // so a row action left the batch buttons live (and vice versa) and the two
  // could race on the same rows.
  const update = useUpdateAnomalyStatus();
  const writing = update.isPending;
  // A row write carries `rowKey`; the batch does not. That is what tells the
  // two apart when placing spinners.
  const pendingRowKey = writing ? update.variables?.rowKey : undefined;
  const pendingBatchStatus = writing && !pendingRowKey ? update.variables?.status : undefined;
  // What each action would actually move. Rows already in the target status
  // are excluded from the write — the server skips them anyway — and from the
  // count the toast reports against, so "3 of 5" means three landed and two had
  // moved on, not that two were already there.
  // Only the two statuses the UI actually offers. Reopening exists on the
  // endpoint and in the SDK, but no surface here writes it, and a filter
  // nothing reads is a filter that quietly rots.
  const actionable = useMemo(
    () => ({
      acknowledged: selectedEvents.filter((e) => canMoveTo(e, "acknowledged", statusFilter)),
      dismissed: selectedEvents.filter((e) => canMoveTo(e, "dismissed", statusFilter))
    }),
    [selectedEvents, statusFilter]
  );
  const applyBulk = (status: "acknowledged" | "dismissed") => {
    const moving = actionable[status];
    if (moving.length === 0) return;
    update.mutate(
      { group: targetOf(moving, statusFilter), status, events: moving.length },
      // Only on a non-empty apply. A failed batch keeps the selection so the
      // user can retry it rather than re-picking 20 rows, and a zero-row write
      // is the same situation from their side — the hook warns instead of
      // celebrating, so wiping the selection under that toast would leave
      // nothing to retry with.
      { onSuccess: (data) => data.updated > 0 && clearSelection() }
    );
  };

  const { data: monitors = [] } = useMonitors();
  const scanMutation = useScanMetricAnomalies();
  const triggerScan = () => {
    setFailuresDismissed(false);
    scanMutation.mutate(asOf || undefined);
  };
  const scanFailures = scanMutation.data?.failures ?? [];

  const goToPage = (next: number) => {
    setPage(next);
    // Selection is page-scoped: the bar acts on what you can see, so carrying
    // keys across a page turn would apply a batch to rows scrolled off screen.
    clearSelection();
  };

  return (
    <div className='flex h-full min-h-0 flex-col overflow-hidden'>
      <Tabs defaultValue='inbox' className='flex h-full min-h-0 flex-col'>
        <TabsList className='w-full justify-start rounded-none border-border border-b bg-transparent px-4 py-0'>
          <TabsTrigger
            value='inbox'
            className='rounded-none border-transparent border-b-2 data-[state=active]:border-b-primary data-[state=active]:bg-transparent data-[state=active]:shadow-none'
          >
            Inbox
          </TabsTrigger>
          <TabsTrigger
            value='monitors'
            className='rounded-none border-transparent border-b-2 data-[state=active]:border-b-primary data-[state=active]:bg-transparent data-[state=active]:shadow-none'
          >
            Monitors
            {monitors.length > 0 && (
              <Badge variant='secondary' className='ml-1.5 rounded-full px-1.5 py-0 text-xs'>
                {monitors.length}
              </Badge>
            )}
          </TabsTrigger>
        </TabsList>

        <TabsContent value='inbox' className='mt-0 flex min-h-0 flex-1 flex-col'>
          <div className='flex items-center justify-between gap-2 border-border border-b px-4 py-2'>
            <div className='flex items-center gap-2'>
              <Select
                value={statusFilter}
                onValueChange={(v) => {
                  setStatusFilter(v as AnomalyStatus | "all");
                  // Each tab is its own list — page 3 of "New" says nothing
                  // about where "Dismissed" starts.
                  setPage(1);
                  clearSelection();
                }}
              >
                <SelectTrigger className='w-36'>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent className={DESIGN_REFERENCE}>
                  {STATUS_OPTIONS.map((opt) => (
                    <SelectItem key={opt.value} value={opt.value}>
                      {opt.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              {/* With no total — the count query failed — fall back to a floor:
                  the pages already behind you plus what this one holds.
                  Counting only this page read "25+" beside a pager saying
                  "Page 3", understating by fifty and contradicting the widget
                  next to it. */}
              {countFloor !== undefined && countFloor > 0 && (
                <span className='text-muted-foreground text-sm'>
                  {countFloor}
                  {totalEvents === undefined && isFullPage ? "+" : ""}{" "}
                  {countFloor === 1 ? "anomaly" : "anomalies"}
                </span>
              )}
            </div>
            <div className='flex items-center gap-2'>
              <label className='flex items-center gap-1 text-muted-foreground text-xs'>
                As of
                <input
                  type='date'
                  value={asOf}
                  onChange={(e) => setAsOf(e.target.value)}
                  className='h-8 border border-input bg-background px-2 text-foreground text-sm'
                />
              </label>
              <Button
                size='sm'
                variant='outline'
                onClick={triggerScan}
                disabled={scanMutation.isPending}
              >
                {scanMutation.isPending ? (
                  <Spinner className='size-4' />
                ) : (
                  <RefreshCw className='size-4' />
                )}
                Scan now
              </Button>
            </div>
          </div>

          {!failuresDismissed && scanFailures.length > 0 && (
            <ScanFailuresBanner
              failures={scanFailures}
              onDismiss={() => setFailuresDismissed(true)}
            />
          )}

          <div className='flex-1 overflow-auto px-4 py-3'>
            {isLoading && (
              <div className='flex h-32 items-center justify-center'>
                <Spinner />
              </div>
            )}
            {/* A failed list has nothing on its way to fix it — `retry: false`
                — so the error state carries its own way out. It sits ABOVE the
                table rather than replacing it: a background refetch can fail
                while React Query still holds the last good page, and throwing
                those rows away would be a worse answer than showing them with
                the failure named. */}
            {error && (
              <div className='flex flex-col items-start gap-2'>
                <span className='text-destructive text-sm'>
                  {error instanceof Error ? error.message : "Failed to load anomalies."}
                </span>
                <div className='flex items-center gap-2'>
                  <Button size='sm' variant='outline' onClick={() => refetch()}>
                    <RefreshCw className='size-4' />
                    Try again
                  </Button>
                  {page > 1 && (
                    <Button size='sm' variant='ghost' onClick={() => goToPage(1)}>
                      Back to first page
                    </Button>
                  )}
                </div>
              </div>
            )}
            {!isLoading && !error && events.length === 0 && (
              <ZeroRows
                // Unknown total on page 1 with no rows is an empty workspace as
                // far as anyone can tell — offering "these moved, refresh"
                // there is a button that can never change the outcome.
                empty={totalEvents === 0 || (totalEvents === undefined && page === 1)}
                settling={isFetching || (totalPages !== undefined && page > totalPages)}
                onScan={triggerScan}
                scanning={scanMutation.isPending}
                onRefresh={() => refetch()}
              />
            )}
            {events.length > 0 && (
              <>
                {selectedEvents.length > 0 && (
                  <BulkActionBar
                    eventCount={selectedEvents.length}
                    actionable={{
                      acknowledged: actionable.acknowledged.length,
                      dismissed: actionable.dismissed.length
                    }}
                    pendingStatus={pendingBatchStatus}
                    // The same expression the table gets. Freezing one and not
                    // the other leaves the bar able to re-send a row's write
                    // while its refetch is still in flight.
                    busy={writing || isPlaceholderData}
                    onApply={applyBulk}
                    onClear={clearSelection}
                  />
                )}
                {/* While the placeholder is up, these rows belong to the page
                    you just left, but the pager below already reads as the new
                    one. Dim and freeze them rather than leave a live table
                    disagreeing with its own "showing 26–50 of N". */}
                <div
                  className={cn(
                    "transition-opacity",
                    isPlaceholderData && "pointer-events-none opacity-50"
                  )}
                  aria-busy={isPlaceholderData}
                >
                  <AnomalyTable
                    events={events}
                    onExplain={setSelectedAnomaly}
                    selected={selectedKeys}
                    onToggle={(key) =>
                      setSelectedKeys((prev) => {
                        const next = new Set(prev);
                        if (!next.delete(key)) next.add(key);
                        return next;
                      })
                    }
                    onToggleAll={(selectAll) =>
                      setSelectedKeys(selectAll ? new Set(events.map((e) => e.key)) : new Set())
                    }
                    onAct={(event, status) =>
                      update.mutate({
                        group: targetOf([event], statusFilter),
                        status,
                        rowKey: event.key
                      })
                    }
                    viewing={statusFilter}
                    pendingRowKey={pendingRowKey}
                    pendingStatus={writing ? update.variables?.status : undefined}
                    // `writing` covers the refetch too: the mutation awaits
                    // its own invalidation, so it stays pending until the rows
                    // it wrote are back.
                    busy={writing || isPlaceholderData}
                  />
                </div>
              </>
            )}
            {/* Outside the rows block, and not gated on `error`: the states
                that need the pager most are the ones that have no rows — an
                emptied page, or a failed refetch over stale data. Gating it on
                either left those with no way out but the status filter. */}
            {!isLoading && data && totalPages !== undefined && (
              <TablePagination
                currentPage={page}
                totalPages={totalPages}
                totalItems={totalEvents ?? 0}
                pageSize={served}
                onPageChange={goToPage}
                itemLabel='anomalies'
              />
            )}
            {/* No total means the server served the page but its count query
                failed. Paging still works — the rows and the offset are real —
                so offer plain steps rather than page numbers we cannot compute,
                instead of hiding navigation over a missing denominator. */}
            {!isLoading && data && totalPages === undefined && (
              <UncountedPager
                page={page}
                // Three ways this is the last page you can reach: the response
                // carries no `offset` at all (a replica predating paging, which
                // ignores it and would serve the same rows forever), the next
                // step would pass the offset the server refuses, or this page
                // came back short. A short page is the ordinary end-of-list
                // signal; it can also be a full page thinned by a concurrent
                // ack, and the cost of that is one Next click you have to make
                // twice — with Previous right there either way.
                atEnd={!echoesOffset || page >= deepestPage || !isFullPage}
                onPageChange={goToPage}
              />
            )}
          </div>
        </TabsContent>

        <TabsContent value='monitors' className='mt-0 flex min-h-0 flex-1 flex-col'>
          <MonitorsTab />
        </TabsContent>
      </Tabs>

      <ExplainDrawer
        anomaly={selectedAnomaly}
        onOpenChange={(open) => {
          if (!open) setSelectedAnomaly(null);
        }}
      />
    </div>
  );
}

/**
 * What to show when the page came back with no rows — three different states,
 * and telling them apart is the whole point:
 *
 *  - `total` is 0: the workspace really has nothing in this filter. Scan CTA.
 *  - a refetch is in flight, or the page is past the end and about to be
 *    clamped: this is a beat between two responses. Spinner.
 *  - neither: `total` and the page disagree with nothing on the way to fix it.
 *    The list endpoint counts and pages in separate, non-transactional
 *    queries, so a concurrent scan (or someone else's bulk-ack) can empty a
 *    still-in-range page. Nothing will re-fetch on its own, so offer the
 *    refresh rather than spin forever.
 */
function ZeroRows({
  empty,
  settling,
  onScan,
  scanning,
  onRefresh
}: {
  empty: boolean;
  settling: boolean;
  onScan: () => void;
  scanning: boolean;
  onRefresh: () => void;
}) {
  if (empty) return <EmptyState onScan={onScan} scanning={scanning} />;
  if (settling) {
    return (
      <div className='flex h-32 items-center justify-center'>
        <Spinner />
      </div>
    );
  }
  return (
    <div className='flex h-32 flex-col items-center justify-center gap-2 text-center'>
      <p className='text-muted-foreground text-sm'>
        These anomalies moved while you were looking at them.
      </p>
      <Button size='sm' variant='outline' onClick={onRefresh}>
        <RefreshCw className='size-4' />
        Refresh
      </Button>
    </div>
  );
}

/**
 * Prev/Next for the case where `total` is missing — the server served the page
 * but its count query failed. Page numbers need a denominator; stepping does
 * not, and the alternative (no pager at all) strands whoever is on page 3.
 */
function UncountedPager({
  page,
  atEnd,
  onPageChange
}: {
  page: number;
  atEnd: boolean;
  onPageChange: (page: number) => void;
}) {
  if (page === 1 && atEnd) return null;
  return (
    <div className='flex items-center justify-between border-t pt-4'>
      <span className='text-muted-foreground text-sm'>Page {page}</span>
      <div className='flex items-center gap-1'>
        <Button
          size='sm'
          variant='ghost'
          disabled={page <= 1}
          onClick={() => onPageChange(page - 1)}
        >
          Previous
        </Button>
        <Button size='sm' variant='ghost' disabled={atEnd} onClick={() => onPageChange(page + 1)}>
          Next
        </Button>
      </div>
    </div>
  );
}

function EmptyState({ onScan, scanning }: { onScan: () => void; scanning: boolean }) {
  return (
    <div className='flex h-48 flex-col items-center justify-center gap-2 text-center'>
      <p className='text-muted-foreground text-sm'>No anomalies in this view.</p>
      <p className='text-muted-foreground text-xs'>
        Drop a <code className='t-code'>.monitor.yml</code> at the workspace root, then run a scan.
      </p>
      <Button size='sm' variant='outline' onClick={onScan} disabled={scanning}>
        {scanning ? <Spinner className='size-4' /> : <RefreshCw className='size-4' />}
        Scan now
      </Button>
    </div>
  );
}

/**
 * Persistent, dismissible banner listing the monitors that errored in the
 * last scan — the toast is transient and truncates, so this is where a user
 * sees *which* monitor failed and *why*. Cleared when the next scan starts.
 */
function ScanFailuresBanner({
  failures,
  onDismiss
}: {
  failures: ScanFailure[];
  onDismiss: () => void;
}) {
  return (
    <div className='mx-4 mt-3 border border-status-warning/40 bg-status-warning/5 px-3 py-2'>
      <div className='flex items-start justify-between gap-2'>
        <div className='flex items-center gap-1.5 font-medium text-status-warning text-sm'>
          <AlertTriangle className='size-4 shrink-0' />
          {failures.length} monitor{failures.length === 1 ? "" : "s"} failed to scan
        </div>
        <Button
          size='icon'
          variant='ghost'
          className='size-6 text-muted-foreground'
          onClick={onDismiss}
          aria-label='Dismiss scan failures'
        >
          <X className='size-4' />
        </Button>
      </div>
      <ul className='mt-1.5 flex flex-col gap-1'>
        {failures.map((f) => {
          const segment = formatSegment(f);
          return (
            <li
              key={`${f.measure}:${f.time_dimension}:${f.granularity}:${f.dimension_key}`}
              className='text-muted-foreground text-xs'
            >
              <span className='font-medium text-foreground'>{f.label || f.measure}</span>
              {segment && <span className='ml-1 text-muted-foreground'>[{segment}]</span>}
              <span className='ml-1'>— {f.error}</span>
            </li>
          );
        })}
      </ul>
    </div>
  );
}
