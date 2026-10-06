import { CircleAlert, X } from "lucide-react";
import { Badge } from "@/components/ui/shadcn/badge";
import { Button } from "@/components/ui/shadcn/button";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import { cn } from "@/libs/shadcn/utils";
import type {
  MonitorEntry,
  MonitorPreview,
  PreviewFlag,
  SegmentPreview
} from "@/types/metricAnomalies";
import { formatNumber } from "@/utils/measureFormat";
import { deviationPercent, previewHeadline } from "../monitorPreview";

type Granularity = MonitorEntry["granularity"];

/** A bucket's name, read the way the inbox reads it — off the UTC date. */
function periodLabel(timestamp: string, granularity: Granularity): string {
  const day = timestamp.slice(0, 10);
  if (granularity === "week") return `Week of ${day}`;
  if (granularity === "month") {
    return new Date(timestamp).toLocaleDateString("en-US", {
      year: "numeric",
      month: "short",
      timeZone: "UTC"
    });
  }
  return day;
}

function Flags({ flags, granularity }: { flags: PreviewFlag[]; granularity: Granularity }) {
  return (
    <div className='grid grid-cols-[max-content_repeat(3,minmax(0,6rem))_max-content] items-baseline gap-x-6 gap-y-1 text-sm'>
      <span className='t-label text-muted-foreground'>Period</span>
      <span className='t-label text-right text-muted-foreground'>Observed</span>
      <span className='t-label text-right text-muted-foreground'>Expected</span>
      <span className='t-label text-right text-muted-foreground'>Δ%</span>
      <span className='t-label text-muted-foreground'>Severity</span>
      {flags.map((flag) => {
        const delta = deviationPercent(flag);
        return (
          <div key={flag.timestamp} className='col-span-full grid grid-cols-subgrid items-baseline'>
            <span className='tabular-nums'>{periodLabel(flag.timestamp, granularity)}</span>
            <span className='t-code text-right'>{formatNumber(flag.observed)}</span>
            <span className='t-code text-right text-muted-foreground'>
              {formatNumber(flag.expected)}
            </span>
            <span className='t-code text-right'>
              {delta === null ? "—" : `${delta > 0 ? "+" : ""}${delta.toFixed(1)}%`}
            </span>
            <span>
              <Badge variant={flag.severity === "high" ? "destructive" : "outline"}>
                {flag.severity}
              </Badge>
            </span>
          </div>
        );
      })}
    </div>
  );
}

/** One segment's part of the answer. A quiet segment has nothing to add to
 *  the headline and renders nothing. */
function Segment({
  segment,
  granularity,
  named
}: {
  segment: SegmentPreview;
  granularity: Granularity;
  named: boolean;
}) {
  if (segment.state === "scored" && segment.flagged.length === 0) return null;
  return (
    <div className='space-y-1.5' data-testid='monitor-preview-segment'>
      {named && (
        <p className='font-mono text-xs'>
          {segment.dimension_key}
          {segment.state === "warming_up" && (
            <span className='ml-3 font-sans text-muted-foreground'>
              not scored yet — {segment.measured_buckets} of {segment.required_buckets}
            </span>
          )}
        </p>
      )}
      {segment.state === "failed" && (
        <pre className='whitespace-pre-wrap break-words rounded bg-muted px-3 py-2 font-mono text-xs'>
          {segment.error}
        </pre>
      )}
      {segment.state === "scored" && <Flags flags={segment.flagged} granularity={granularity} />}
    </div>
  );
}

/**
 * The answer to "what would a scan make of this monitor", under its row.
 *
 * Reads top to bottom as the answer does: one sentence, then the evidence for
 * it, then the reminder that asking changed nothing.
 */
export function MonitorPreviewPanel({
  granularity,
  preview,
  isPending,
  error,
  onClose
}: {
  granularity: Granularity;
  preview: MonitorPreview | undefined;
  isPending: boolean;
  error: Error | null;
  onClose: () => void;
}) {
  const headline = preview ? previewHeadline(preview, granularity) : null;
  const named = (preview?.segments_total ?? 1) > 1;
  return (
    <div className='space-y-3 bg-muted/40 px-4 py-3 text-sm' data-testid='monitor-preview'>
      <div className='flex items-baseline gap-4'>
        <span className='t-label w-16 shrink-0 text-muted-foreground'>Preview</span>
        <div className='min-w-0 flex-1'>
          {isPending && <Skeleton className='h-4 w-72' />}
          {error && (
            <p className='flex items-start gap-2 text-destructive'>
              <CircleAlert className='mt-0.5 size-4 shrink-0' aria-hidden />
              {error.message}
            </p>
          )}
          {headline && (
            <p
              className={cn(
                "font-medium",
                headline.tone === "failed" && "text-destructive",
                headline.tone === "quiet" && "font-normal"
              )}
              data-testid='monitor-preview-headline'
            >
              {headline.text}
            </p>
          )}
        </div>
        <Button
          variant='ghost'
          size='icon'
          className='size-6 shrink-0 self-center'
          onClick={onClose}
          aria-label='Close preview'
        >
          <X className='size-3.5' />
        </Button>
      </div>
      {preview && (
        <div className='ml-20 space-y-3'>
          {preview.segments.map((segment) => (
            <Segment
              key={segment.dimension_key}
              segment={segment}
              granularity={granularity}
              named={named}
            />
          ))}
          <p className='text-muted-foreground text-xs'>
            Nothing was written — no inbox rows, no run, no Slack post.
          </p>
        </div>
      )}
    </div>
  );
}
