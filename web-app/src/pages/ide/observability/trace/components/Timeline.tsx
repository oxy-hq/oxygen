import { cn } from "@/libs/shadcn/utils";
import type { TimelineSpan } from "@/services/api/traces";
import { formatDuration } from "../../utils/index";
import { isEmptyToolSpan, LEGEND_CATEGORIES, SPAN_CATEGORY_META } from "./spanCategory";
import { TimelineSpanRow } from "./TimelineSpanRow";

interface TimelineProps {
  spans: TimelineSpan[];
  totalDuration: number;
  selectedSpanId?: string;
  onSelectSpan: (span: TimelineSpan) => void;
  selfTimes: Map<string, number>;
  criticalPath: Set<string>;
}

function CategoryLegend() {
  return (
    <div className='flex flex-wrap items-center gap-x-3 gap-y-1 text-muted-foreground text-xs'>
      {LEGEND_CATEGORIES.map((category) => (
        <span key={category} className='flex items-center gap-1.5'>
          <span className={cn("h-2 w-2 rounded-sm", SPAN_CATEGORY_META[category].dotClass)} />
          {SPAN_CATEGORY_META[category].label}
        </span>
      ))}
      <span className='flex items-center gap-1.5'>
        <span className='h-2 w-3 rounded-sm outline outline-dashed outline-1 outline-primary/70' />
        critical path
      </span>
    </div>
  );
}

export function Timeline({
  spans,
  totalDuration,
  selectedSpanId,
  onSelectSpan,
  selfTimes,
  criticalPath
}: TimelineProps) {
  const filteredSpans = spans.filter((s) => !isEmptyToolSpan(s));
  const rootSpans = filteredSpans.filter((s) => !s.parentSpanId);

  return (
    <div className='min-w-fit'>
      {/* Timeline header - sticky */}
      <div className='sticky top-0 z-10 border-b bg-background/95 backdrop-blur-sm'>
        <div className='flex items-center gap-4 px-4 py-2'>
          <div className='flex flex-1 items-center font-medium text-muted-foreground text-xs'>
            <div className='w-6' /> {/* Expand button space */}
            <div className='w-64 flex-shrink-0 pl-2'>Span · self-time</div>
            <div className='flex min-w-[200px] flex-1 justify-between px-3'>
              <span>0ms</span>
              <span>{formatDuration(totalDuration / 2)}</span>
              <span>{formatDuration(totalDuration)}</span>
            </div>
            <div className='w-20 pr-2 text-right'>Duration</div>
          </div>
        </div>
        <div className='px-4 pb-2'>
          <CategoryLegend />
        </div>
      </div>

      {/* Spans */}
      <div className='py-1'>
        {rootSpans.map((span, index) => (
          <TimelineSpanRow
            key={span.spanId}
            span={span}
            spans={filteredSpans}
            totalDuration={totalDuration}
            selectedSpanId={selectedSpanId}
            onSelectSpan={onSelectSpan}
            selfTimes={selfTimes}
            criticalPath={criticalPath}
            ancestorHasMoreSiblings={[]}
            isLastChild={index === rootSpans.length - 1}
          />
        ))}
      </div>
    </div>
  );
}
