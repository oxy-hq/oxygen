import { AlertCircle, ArrowDown, ArrowRight, ArrowUp, CheckCircle2 } from "lucide-react";
import { useMemo } from "react";
import { Badge } from "@/components/ui/shadcn/badge";
import { Button } from "@/components/ui/shadcn/button";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/shadcn/dialog";
import { Spinner } from "@/components/ui/shadcn/spinner";
import useTraceDetail from "@/hooks/api/traces/useTraceDetail";
import type { Trace } from "@/services/api/traces";
import { TraceSummaryStrip } from "../../trace/components/TraceSummaryStrip";
import { summarizeTrace, type TraceSummary } from "../../trace/components/traceSummary";
import { formatDuration, formatTimeAgo } from "../../utils";
import { deriveTraceRow } from "./traceRow";

/** What Compare needs of one side: the strip's figures and the wall time. */
interface ComparedTrace {
  summary: TraceSummary;
  totalDurationMs: number;
}

/** One side of the comparison, summarised from that trace's own spans. */
function useComparedTrace(trace: Trace | undefined, enabled: boolean) {
  const { data, isLoading } = useTraceDetail(trace?.traceId ?? "", enabled && !!trace);
  const compared = useMemo<ComparedTrace | undefined>(
    () =>
      data
        ? { summary: summarizeTrace(data.spans), totalDurationMs: data.totalDurationMs }
        : undefined,
    [data]
  );
  return { compared, isLoading };
}

interface CompareTracesDialogProps {
  traces: Trace[];
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onOpenTrace: (traceId: string) => void;
}

function ColumnHeader({
  trace,
  label,
  onOpenTrace
}: {
  trace: Trace;
  label: string;
  onOpenTrace: (traceId: string) => void;
}) {
  const row = deriveTraceRow(trace);
  return (
    <div className='flex flex-col gap-1.5'>
      <div className='flex items-center gap-2'>
        <Badge variant='secondary' className='text-xs'>
          {label}
        </Badge>
        {row.isError ? (
          <AlertCircle className='size-4 text-destructive' />
        ) : (
          <CheckCircle2 className='size-4 text-success' />
        )}
        <span className='truncate font-medium text-sm' title={row.title}>
          {row.title}
        </span>
      </div>
      <div className='flex items-center gap-2 text-muted-foreground text-xs'>
        <Badge variant='outline' className='text-xs'>
          {row.spanLabel}
        </Badge>
        {row.entityRef && <span className='truncate'>{row.entityRef}</span>}
        <span>{formatTimeAgo(row.timestamp)}</span>
        <Button
          variant='link'
          size='sm'
          className='h-auto p-0 text-xs'
          onClick={() => onOpenTrace(row.traceId)}
        >
          Open
        </Button>
      </div>
    </div>
  );
}

function SummaryPanel({
  trace,
  label,
  compared,
  isLoading,
  onOpenTrace
}: {
  trace: Trace;
  label: string;
  compared?: ComparedTrace;
  isLoading: boolean;
  onOpenTrace: (traceId: string) => void;
}) {
  return (
    <div className='flex min-w-0 flex-col gap-3'>
      <ColumnHeader trace={trace} label={label} onOpenTrace={onOpenTrace} />
      {isLoading ? (
        <div className='flex h-32 items-center justify-center'>
          <Spinner className='size-6 text-muted-foreground' />
        </div>
      ) : compared ? (
        <TraceSummaryStrip summary={compared.summary} totalDurationMs={compared.totalDurationMs} />
      ) : (
        <p className='text-muted-foreground text-sm'>Could not load trace summary.</p>
      )}
    </div>
  );
}

interface DeltaRow {
  label: string;
  delta: number;
  formatted: string;
}

function buildDeltas(a: ComparedTrace, b: ComparedTrace): DeltaRow[] {
  const fmtNum = (n: number) => (n >= 0 ? "+" : "−") + Math.abs(n).toLocaleString();
  return [
    {
      label: "Duration",
      delta: b.totalDurationMs - a.totalDurationMs,
      formatted:
        (b.totalDurationMs - a.totalDurationMs >= 0 ? "+" : "−") +
        formatDuration(Math.abs(b.totalDurationMs - a.totalDurationMs))
    },
    {
      label: "Tokens",
      delta: b.summary.totalTokens - a.summary.totalTokens,
      formatted: fmtNum(b.summary.totalTokens - a.summary.totalTokens)
    },
    {
      label: "Spans",
      delta: b.summary.spanCount - a.summary.spanCount,
      formatted: fmtNum(b.summary.spanCount - a.summary.spanCount)
    },
    {
      label: "Errors",
      delta: b.summary.errorCount - a.summary.errorCount,
      formatted: fmtNum(b.summary.errorCount - a.summary.errorCount)
    }
  ];
}

function DeltaStrip({ a, b }: { a: ComparedTrace; b: ComparedTrace }) {
  return (
    <div className='rounded-lg border bg-muted/40 p-3'>
      <div className='mb-2 flex items-center gap-1 text-muted-foreground text-xs uppercase tracking-wide'>
        Difference <ArrowRight className='size-3' /> B − A
      </div>
      <div className='grid grid-cols-2 gap-2 sm:grid-cols-4'>
        {buildDeltas(a, b).map((d) => (
          <div key={d.label} className='flex flex-col gap-0.5'>
            <span className='text-[10px] text-muted-foreground uppercase tracking-wide'>
              {d.label}
            </span>
            <span className='flex items-center gap-1 font-semibold text-sm tabular-nums'>
              {d.delta !== 0 &&
                (d.delta > 0 ? (
                  <ArrowUp className='size-3 text-muted-foreground' />
                ) : (
                  <ArrowDown className='size-3 text-muted-foreground' />
                ))}
              {d.delta === 0 ? "—" : d.formatted}
            </span>
          </div>
        ))}
      </div>
    </div>
  );
}

/** Side-by-side comparison of two traces' summary metrics (Theme 3e). */
export function CompareTracesDialog({
  traces,
  open,
  onOpenChange,
  onOpenTrace
}: CompareTracesDialogProps) {
  const [a, b] = traces;
  const sideA = useComparedTrace(a, open);
  const sideB = useComparedTrace(b, open);

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className='max-w-4xl'>
        <DialogHeader>
          <DialogTitle>Compare traces</DialogTitle>
        </DialogHeader>
        {a && b && (
          <div className='flex flex-col gap-4'>
            <div className='grid grid-cols-1 gap-4 sm:grid-cols-2'>
              <SummaryPanel
                trace={a}
                label='A'
                compared={sideA.compared}
                isLoading={sideA.isLoading}
                onOpenTrace={onOpenTrace}
              />
              <SummaryPanel
                trace={b}
                label='B'
                compared={sideB.compared}
                isLoading={sideB.isLoading}
                onOpenTrace={onOpenTrace}
              />
            </div>
            {sideA.compared && sideB.compared && (
              <DeltaStrip a={sideA.compared} b={sideB.compared} />
            )}
          </div>
        )}
      </DialogContent>
    </Dialog>
  );
}
