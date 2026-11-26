import { ChevronDown, ChevronUp } from "lucide-react";
import { useMemo } from "react";
import { Badge } from "@/components/ui/shadcn/badge";
import { Button } from "@/components/ui/shadcn/button";
import { Checkbox } from "@/components/ui/shadcn/checkbox";
import { Spinner } from "@/components/ui/shadcn/spinner";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow
} from "@/components/ui/shadcn/table";
import type {
  AnomalyFilter,
  AnomalySeverity,
  AnomalyStatus,
  MetricAnomaly
} from "@/types/metricAnomalies";

import { formatNumber } from "@/utils/measureFormat";
import { type AnomalyEvent, canMoveTo } from "./events";

export default function AnomalyTable({
  events,
  onExplain,
  selected,
  onToggle,
  onToggleAll,
  onAct,
  viewing,
  pendingRowKey,
  pendingStatus,
  busy
}: {
  events: AnomalyEvent[];
  onExplain: (a: MetricAnomaly) => void;
  /** Keys of the currently selected events (see {@link AnomalyEvent.key}). */
  selected: Set<string>;
  onToggle: (key: string) => void;
  /** `true` selects every event on the page, `false` clears them. The header
   *  checkbox already knows which way it is going, so the parent doesn't
   *  re-derive it — two copies of that predicate would be one to drift. */
  onToggleAll: (selectAll: boolean) => void;
  /** Act on one row. The write lives with the page, not here, so a row action
   *  and the batch bar share one in-flight state instead of each seeing only
   *  its own and racing on the same rows. */
  onAct: (event: AnomalyEvent, status: AnomalyStatus) => void;
  /** The status filter these rows were listed under. It decides which of an
   *  event's buckets a write may touch, so the buttons have to ask with it —
   *  see `canMoveTo`. */
  viewing: AnomalyStatus | "all";
  /** The row currently being written, if any, and the status it is moving to —
   *  together they place the spinner on the button that was clicked. */
  pendingRowKey: string | undefined;
  pendingStatus: AnomalyStatus | undefined;
  /** Any write is in flight, or these rows belong to the page being left.
   *  Either way nothing here may be re-targeted until it settles. */
  busy: boolean;
}) {
  const allSelected = events.length > 0 && events.every((e) => selected.has(e.key));
  const someSelected = events.some((e) => selected.has(e.key));

  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead className='w-8'>
            <Checkbox
              // Indeterminate, not unchecked: a partial page selection must not
              // read as "nothing selected" when the bar below says otherwise.
              checked={allSelected ? true : someSelected ? "indeterminate" : false}
              onCheckedChange={() => onToggleAll(!allSelected)}
              disabled={busy}
              aria-label='Select every anomaly on this page'
            />
          </TableHead>
          <TableHead className='w-24'>Severity</TableHead>
          <TableHead>Measure</TableHead>
          <TableHead>Period</TableHead>
          <TableHead className='text-right'>Observed</TableHead>
          <TableHead className='text-right'>Expected</TableHead>
          <TableHead className='text-right'>Δ%</TableHead>
          <TableHead className='w-72 text-right'>Actions</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {events.map((event) => (
          <AnomalyRow
            key={event.key}
            event={event}
            onExplain={onExplain}
            selected={selected.has(event.key)}
            onToggle={() => onToggle(event.key)}
            onAct={(status) => onAct(event, status)}
            viewing={viewing}
            pendingStatus={pendingRowKey === event.key ? pendingStatus : undefined}
            busy={busy}
          />
        ))}
      </TableBody>
    </Table>
  );
}

function AnomalyRow({
  event,
  onExplain,
  selected,
  onToggle,
  onAct,
  viewing,
  pendingStatus,
  busy
}: {
  event: AnomalyEvent;
  onExplain: (a: MetricAnomaly) => void;
  selected: boolean;
  onToggle: () => void;
  /** Status actions apply to the whole event: acknowledging "the surge" and
   *  leaving its other two days in the inbox would defeat the grouping. The
   *  table owns the write, so one in-flight action freezes every row rather
   *  than only its own. */
  onAct: (status: AnomalyStatus) => void;
  viewing: AnomalyStatus | "all";
  /** The status this row is currently writing, so the spinner lands on the
   *  button that was clicked and not on both. */
  pendingStatus: AnomalyStatus | undefined;
  busy: boolean;
}) {
  const anomaly = event.peak;

  const deltaPct = useMemo(() => {
    if (Math.abs(anomaly.expected) < 1e-9) return null;
    return ((anomaly.observed - anomaly.expected) / Math.abs(anomaly.expected)) * 100;
  }, [anomaly.expected, anomaly.observed]);

  return (
    <TableRow data-status={event.status} data-state={selected ? "selected" : undefined}>
      <TableCell>
        <Checkbox
          checked={selected}
          onCheckedChange={onToggle}
          disabled={busy}
          aria-label={`Select ${anomaly.label || anomaly.measure}`}
        />
      </TableCell>
      <TableCell>
        <div className='flex flex-col items-start gap-1'>
          <SeverityBadge severity={event.severity} status={event.status} />
          {/* In the "All" view the row stays put after an Ack, so without this
              the only sign the action landed is a button disappearing. The
              event's status, not the peak bucket's — see `AnomalyEvent`. */}
          {event.status !== "new" && (
            <span className='text-muted-foreground text-xs capitalize'>{event.status}</span>
          )}
        </div>
      </TableCell>
      <TableCell>
        <div className='flex flex-col gap-1'>
          <span className='font-medium'>{anomaly.label || anomaly.measure}</span>
          <span className='text-muted-foreground text-xs'>{anomaly.measure}</span>
          <SegmentBadge anomaly={anomaly} />
        </div>
      </TableCell>
      <TableCell className='text-sm'>
        <div className='flex flex-col'>
          <span>{formatPeriod(anomaly)}</span>
          {event.buckets.length > 1 ? (
            <span className='text-muted-foreground text-xs'>
              {/* `+` only when the server says it trimmed this event. Guessing
                  from the count instead would mark a complete 50-bucket event,
                  and miss that the cap applies after the status filter. */}
              {anomaly.granularity} · worst of {event.buckets.length}
              {event.truncated && "+"} in this event
            </span>
          ) : (
            <span className='text-muted-foreground text-xs'>{anomaly.granularity}</span>
          )}
        </div>
      </TableCell>
      <TableCell className='t-code text-right'>{formatNumber(anomaly.observed)}</TableCell>
      <TableCell className='t-code text-right'>{formatNumber(anomaly.expected)}</TableCell>
      <TableCell className='t-code text-right'>
        {deltaPct === null ? "—" : <DeltaArrow value={deltaPct} />}
      </TableCell>
      <TableCell className='text-right'>
        <div className='flex justify-end gap-1'>
          {/* Deliberately not `disabled={busy}`, unlike the two beside it. The
              freeze exists so a second write can't race the first over rows a
              refetch is still settling; opening a read-only drawer races
              nothing, and its decomposition takes 20-30s, so blocking it
              through someone else's ack costs real waiting to buy symmetry. */}
          <Button size='sm' variant='default' onClick={() => onExplain(anomaly)}>
            Explain
          </Button>
          {canMoveTo(event, "acknowledged", viewing) && (
            <Button
              size='sm'
              variant='outline'
              disabled={busy}
              onClick={() => onAct("acknowledged")}
            >
              {pendingStatus === "acknowledged" && <Spinner className='size-4' />}
              Ack
            </Button>
          )}
          {canMoveTo(event, "dismissed", viewing) && (
            <Button size='sm' variant='ghost' disabled={busy} onClick={() => onAct("dismissed")}>
              {pendingStatus === "dismissed" && <Spinner className='size-4' />}
              Dismiss
            </Button>
          )}
        </div>
      </TableCell>
    </TableRow>
  );
}

/**
 * Chain-wide monitors have no segment; segment (`group_by` / filtered)
 * monitors render a chip so per-segment anomalies that share a measure/period
 * are visibly distinct instead of reading as duplicates.
 */
function SegmentBadge({ anomaly }: { anomaly: MetricAnomaly }) {
  const label = formatSegment(anomaly);
  if (!label) return null;
  return (
    <Badge variant='secondary' className='w-fit font-normal text-xs'>
      {label}
    </Badge>
  );
}

/** e.g. filters `[{member: "labor_daily.restaurant_id", values: ["loc-abc"]}]`
 *  → `"restaurant_id: loc-abc"`. Falls back to the raw `dimension_key`.
 *  Shared by anomaly rows and the scan-failures banner so the same segment
 *  reads identically on both surfaces. */
export function formatSegment(segment: {
  filters: AnomalyFilter[] | null;
  dimension_key: string;
}): string | null {
  const { filters, dimension_key } = segment;
  if (filters && filters.length > 0) {
    return filters.map((f) => `${shortMember(f.member)}: ${f.values.join(", ")}`).join(" · ");
  }
  return dimension_key ? dimension_key : null;
}

/** Strip the `view.` prefix from a fully-qualified dimension id. */
function shortMember(member: string): string {
  const dot = member.lastIndexOf(".");
  return dot === -1 ? member : member.slice(dot + 1);
}

function SeverityBadge({ severity, status }: { severity: AnomalySeverity; status: AnomalyStatus }) {
  const tone = status === "dismissed" ? "muted" : severityTone(severity);
  return (
    <Badge variant={tone === "destructive" ? "destructive" : "outline"} className={toneClass(tone)}>
      {severity}
    </Badge>
  );
}

type Tone = "destructive" | "warning" | "info" | "muted";

function severityTone(severity: AnomalySeverity): Tone {
  switch (severity) {
    case "high":
      return "destructive";
    case "medium":
      return "warning";
    case "low":
      return "info";
  }
}

function toneClass(tone: Tone): string {
  switch (tone) {
    case "destructive":
      return "";
    case "warning":
      return "border-status-warning/40 text-status-warning";
    case "info":
      return "border-status-info/40 text-status-info";
    case "muted":
      return "border-muted text-muted-foreground";
  }
}

function DeltaArrow({ value }: { value: number }) {
  const positive = value >= 0;
  const Icon = positive ? ChevronUp : ChevronDown;
  // `text-success`, not an emerald ramp: `product-context.md` reserves emerald
  // for workflow-node success, and the token already carries both themes.
  const colorClass = positive ? "text-success" : "text-destructive";
  return (
    <span className={`inline-flex items-center justify-end gap-0.5 ${colorClass}`}>
      <Icon className='size-3' />
      {Math.abs(value).toFixed(1)}%
    </span>
  );
}

function formatPeriod(a: MetricAnomaly): string {
  const start = new Date(a.period_start);
  if (a.granularity === "day") {
    return start.toISOString().slice(0, 10);
  }
  if (a.granularity === "week") {
    return `Week of ${start.toISOString().slice(0, 10)}`;
  }
  return start.toLocaleDateString("en-US", { year: "numeric", month: "short" });
}
