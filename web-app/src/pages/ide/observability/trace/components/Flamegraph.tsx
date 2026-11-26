import { cn } from "@/libs/shadcn/utils";
import type { TimelineSpan } from "@/services/api/traces";
import { formatDuration, formatSpanLabel } from "../../utils/index";
import { getSpanCategory, isErrorStatus, SPAN_CATEGORY_META } from "./spanCategory";

interface FlamegraphProps {
  spans: TimelineSpan[];
  totalDuration: number;
  selectedSpanId?: string;
  onSelectSpan: (span: TimelineSpan) => void;
}

/**
 * Depth-stacked flamegraph over the same span set as the waterfall: one row per
 * depth, each cell positioned by start offset with width ∝ duration.
 */
export function Flamegraph({
  spans,
  totalDuration,
  selectedSpanId,
  onSelectSpan
}: FlamegraphProps) {
  const maxDepth = spans.reduce((max, s) => Math.max(max, s.depth), 0);
  const depths = Array.from({ length: maxDepth + 1 }, (_, d) => d);

  return (
    <div className='min-w-[480px] space-y-1 p-3'>
      {depths.map((depth) => (
        <div key={depth} className='relative h-7'>
          {spans
            .filter((s) => s.depth === depth)
            .map((span) => {
              const left = (span.offsetMs / totalDuration) * 100;
              const width = Math.max((span.durationMs / totalDuration) * 100, 0.4);
              const { barClass } = SPAN_CATEGORY_META[getSpanCategory(span)];
              const isSelected = selectedSpanId === span.spanId;
              const isError = isErrorStatus(span.statusCode);
              return (
                <button
                  key={span.spanId}
                  type='button'
                  onClick={() => onSelectSpan(span)}
                  title={`${span.spanName} · ${formatDuration(span.durationMs)}`}
                  className={cn(
                    "absolute inset-y-0 flex items-center overflow-hidden px-1.5 font-mono text-[10px] text-white transition-[filter] hover:brightness-110",
                    barClass,
                    isError && "ring-1 ring-destructive",
                    isSelected && "outline outline-2 outline-primary outline-offset-1"
                  )}
                  style={{ left: `${Math.max(0, left)}%`, width: `${width}%` }}
                >
                  <span className='truncate'>
                    {formatSpanLabel(span.spanName)} {formatDuration(span.durationMs)}
                  </span>
                </button>
              );
            })}
        </div>
      ))}
    </div>
  );
}
