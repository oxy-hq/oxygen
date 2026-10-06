import { CircleAlert } from "lucide-react";
import { type ReactNode, useMemo } from "react";
import { Badge } from "@/components/ui/shadcn/badge";
import { Button } from "@/components/ui/shadcn/button";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow
} from "@/components/ui/shadcn/table";
import {
  useMetricAnomalies,
  useMonitorCoverage,
  useMonitorNotify,
  useMonitorPreview,
  useMonitors
} from "@/hooks/api/useMetricAnomalies";
import { cn } from "@/libs/shadcn/utils";
import type { MonitorCoverage, MonitorEntry, MonitorNotify } from "@/types/metricAnomalies";
import { MonitorPreviewPanel } from "./components/MonitorPreviewPanel";
import { announcedSeverity } from "./monitorNotify";
import {
  coverageFor,
  filterKey,
  relativeTime,
  sensitivityVariant,
  warmingSummary
} from "./monitorRows";

const inlineCode = "rounded bg-muted px-1.5 py-px font-mono text-xs";

/** The band every state of the tab opens with: a label and one sentence. The
 *  table's rules run from it, so it reads as the table's own first line. */
function Band({
  label,
  children,
  testId
}: {
  label: string;
  children: ReactNode;
  testId?: string;
}) {
  return (
    <div
      className='flex items-baseline gap-4 border-b bg-muted/40 px-4 py-3 text-sm'
      data-testid={testId}
    >
      <span className='t-label w-16 shrink-0 text-muted-foreground'>{label}</span>
      <p className='min-w-0 text-foreground'>{children}</p>
    </div>
  );
}

/** Where new insights go besides this inbox. Said either way: an inbox nobody
 *  is told about and one that posts to a channel look identical otherwise. */
function DeliveryNote({ notify }: { notify: MonitorNotify | null }) {
  return (
    <Band label='Delivery' testId='monitors-delivery-note'>
      {notify ? (
        <>
          New insights <span className='font-medium'>{announcedSeverity(notify)}</span> are posted
          to Slack channel <code className={inlineCode}>{notify.slack_channel}</code>, once each.
        </>
      ) : (
        <>
          New insights appear <span className='font-medium'>only in this inbox</span>. Add a{" "}
          <code className={inlineCode}>notify:</code> block to{" "}
          <code className={inlineCode}>.monitor.yml</code> to post them to a Slack channel.
        </>
      )}
    </Band>
  );
}

/** A cell that says two things: what a person calls it, and what the file
 *  calls it. The second line is always the machine's spelling, in mono. */
function TwoLine({ first, second, mono }: { first: string; second?: string; mono?: boolean }) {
  return (
    <>
      <p className={cn("font-medium", mono && "font-mono")}>{first}</p>
      {second && <p className='font-mono text-muted-foreground text-xs'>{second}</p>}
    </>
  );
}

/** Columns of the table; the preview under a row spans all of them. */
const COLUMNS = 6;

function MonitorRow({
  monitor: m,
  coverage,
  lastAt
}: {
  monitor: MonitorEntry;
  coverage: MonitorCoverage[];
  lastAt: string | undefined;
}) {
  const warming = warmingSummary(coverageFor(m, coverage));
  // One dry run per row, kept under the row that asked for it. Idle until the
  // button is pressed: a preview costs a warehouse query.
  const preview = useMonitorPreview();
  const shown = !preview.isIdle;
  const run = () =>
    preview.mutate({
      measure: m.measure,
      time_dimension: m.time_dimension,
      granularity: m.granularity,
      dimension_key: filterKey(m.filters),
      group_by: m.group_by ?? null
    });
  return (
    <>
      <TableRow className={cn("align-top", shown && "border-b-0")}>
        <TableCell className='py-2.5 pl-4'>
          {/* An unlabelled monitor has only its measure to go by, so the measure
              is the headline and is not repeated beneath itself. */}
          {m.label ? (
            <TwoLine first={m.label} second={m.measure} />
          ) : (
            <TwoLine first={m.measure} mono />
          )}
        </TableCell>
        <TableCell className='py-2.5'>
          <p>{m.granularity}</p>
          <p className='font-mono text-muted-foreground text-xs'>{m.time_dimension}</p>
        </TableCell>
        <TableCell className='py-2.5'>
          <Badge
            variant={sensitivityVariant(m.sensitivity)}
            className={cn(m.sensitivity === "low" && "text-muted-foreground")}
          >
            {m.sensitivity}
          </Badge>
        </TableCell>
        <TableCell className='py-2.5'>
          {warming ? (
            <>
              <Badge variant='secondary'>{warming.label}</Badge>
              <p className='mt-1 flex flex-wrap gap-x-3 text-muted-foreground text-xs tabular-nums'>
                {warming.detail.map((fact) => (
                  <span key={fact}>{fact}</span>
                ))}
              </p>
            </>
          ) : (
            <span className='text-muted-foreground'>—</span>
          )}
        </TableCell>
        <TableCell className='py-2.5 text-right tabular-nums'>
          {lastAt ? relativeTime(lastAt) : <span className='text-muted-foreground'>—</span>}
        </TableCell>
        <TableCell className='py-2 pr-4 text-right'>
          <Button
            variant='outline'
            size='sm'
            onClick={run}
            disabled={preview.isPending}
            data-testid='monitor-preview-run'
          >
            {preview.isPending ? "Running…" : "Preview"}
          </Button>
        </TableCell>
      </TableRow>
      {shown && (
        <TableRow className='hover:bg-transparent'>
          <TableCell colSpan={COLUMNS} className='p-0'>
            <MonitorPreviewPanel
              granularity={m.granularity}
              preview={preview.data}
              isPending={preview.isPending}
              error={preview.error}
              onClose={preview.reset}
            />
          </TableCell>
        </TableRow>
      )}
    </>
  );
}

function SkeletonRow() {
  return (
    <div className='flex items-center gap-6 border-b px-4 py-3'>
      <div className='flex-1 space-y-2'>
        <Skeleton className='h-3.5 w-40' />
        <Skeleton className='h-3 w-56' />
      </div>
      <Skeleton className='h-3.5 w-16' />
      <Skeleton className='h-3.5 w-20' />
    </div>
  );
}

function LoadingState() {
  return (
    <div data-testid='monitors-loading'>
      <Band label='Delivery'>Loading monitors…</Band>
      <SkeletonRow />
      <SkeletonRow />
    </div>
  );
}

export default function MonitorsTab() {
  const { data: monitors = [], isLoading, error } = useMonitors();
  const { data: coverage = [] } = useMonitorCoverage();
  const { data: notify = null } = useMonitorNotify();
  // Fetch anomalies latest-first (no status filter), then take the max
  // `period_start` (the anomaly's bucket date — what the column shows) per
  // measure across all statuses. `order="recent"` is required: the Inbox's
  // default severity ranking would let a measure whose recent anomalies are all
  // `low` fall off the page and show a stale "last anomaly".
  const { data: recent } = useMetricAnomalies(undefined, "recent");

  const lastAnomalyByMeasure = useMemo(() => {
    const map = new Map<string, string>();
    // Depend on the response, not a defaulted `?? []`: that literal is a new
    // array identity every render, which would rebuild this map every time.
    for (const a of recent?.anomalies ?? []) {
      const existing = map.get(a.measure);
      if (!existing || a.period_start > existing) {
        map.set(a.measure, a.period_start);
      }
    }
    return map;
  }, [recent]);

  // Coverage is per segment; the table is per monitor entry. Group on the same
  // (measure, time_dimension, granularity) triple the scanner keys its rows by
  // — granularity included because a daily and a weekly monitor over the same
  // measure have different floors (56 buckets vs 26) and must not be merged.
  // The triple alone does not identify an entry, so `coverageFor` narrows
  // further by filters at lookup time.
  const coverageByMonitor = useMemo(() => {
    const map = new Map<string, MonitorCoverage[]>();
    for (const c of coverage) {
      const key = `${c.measure}:${c.time_dimension}:${c.granularity}`;
      const existing = map.get(key);
      if (existing) existing.push(c);
      else map.set(key, [c]);
    }
    return map;
  }, [coverage]);

  if (isLoading) return <LoadingState />;

  if (error) {
    return (
      <div className='flex items-start gap-2 p-4 text-destructive text-sm'>
        <CircleAlert className='mt-0.5 size-4 shrink-0' aria-hidden />
        <p>{error instanceof Error ? error.message : "Failed to load monitor config."}</p>
      </div>
    );
  }

  if (monitors.length === 0) {
    return (
      <div className='space-y-1 p-4 text-sm'>
        <p className='font-medium'>No monitors configured.</p>
        <p className='text-muted-foreground'>
          Drop a <code className={inlineCode}>.monitor.yml</code> at the workspace root to start.
        </p>
      </div>
    );
  }

  return (
    <div className='flex-1 overflow-auto'>
      <DeliveryNote notify={notify} />
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead className='pl-4'>Metric / measure</TableHead>
            <TableHead className='w-56'>Granularity / time dimension</TableHead>
            <TableHead className='w-28'>Sensitivity</TableHead>
            <TableHead className='w-60'>Coverage</TableHead>
            <TableHead className='w-32 text-right'>Last anomaly</TableHead>
            <TableHead className='w-28 pr-4'>
              <span className='sr-only'>Preview</span>
            </TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {monitors.map((m) => (
            // Two entries can share a measure and time-dimension and differ
            // only by granularity or filters, so the key carries both. That is
            // the full identity of an entry — a .monitor.yml that repeats one
            // verbatim declares the same monitor twice.
            <MonitorRow
              key={`${m.measure}:${m.time_dimension}:${m.granularity}:${filterKey(m.filters)}`}
              monitor={m}
              coverage={
                coverageByMonitor.get(`${m.measure}:${m.time_dimension}:${m.granularity}`) ?? []
              }
              lastAt={lastAnomalyByMeasure.get(m.measure)}
            />
          ))}
        </TableBody>
      </Table>
    </div>
  );
}
