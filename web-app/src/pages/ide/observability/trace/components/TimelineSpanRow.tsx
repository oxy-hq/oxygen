import { AlertCircle, ChevronDown, ChevronRight } from "lucide-react";
import { useState } from "react";
import { cn } from "@/libs/shadcn/utils";
import type { TimelineSpan } from "@/services/api/traces";
import { formatDuration, formatSpanLabel, SpanIcon } from "../../utils/index";
import { getSpanCategory, isErrorStatus, SPAN_CATEGORY_META } from "./spanCategory";
import { getSpanError, getSpanRows, getSpanTokens } from "./spanInspect";

interface TimelineSpanRowProps {
  span: TimelineSpan;
  spans: TimelineSpan[];
  totalDuration: number;
  selectedSpanId?: string;
  onSelectSpan: (span: TimelineSpan) => void;
  /** spanId → self time (ms), for the lighter "in-children" cap on each bar. */
  selfTimes: Map<string, number>;
  /** spanIds on the critical path, highlighted with a dashed outline. */
  criticalPath: Set<string>;
  /** Array of booleans indicating if ancestor at each depth has more siblings after this branch */
  ancestorHasMoreSiblings?: boolean[];
  /** Whether this span is the last child among its siblings */
  isLastChild?: boolean;
}

function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return n.toString();
}

/** Technical payload shown inline on the row: tokens on LLM spans, rows/errors on SQL spans. */
function spanMeta(span: TimelineSpan): { label: string; isError: boolean } | null {
  const category = getSpanCategory(span);
  if (isErrorStatus(span.statusCode)) {
    const err = getSpanError(span);
    if (err) return { label: `✕ ${err}`, isError: true };
  }
  if (category === "llm") {
    const tokens = getSpanTokens(span);
    if (tokens) return { label: `${formatTokens(tokens.total)} tok`, isError: false };
  }
  if (category === "sql") {
    const rows = getSpanRows(span);
    if (rows !== undefined) return { label: `${rows} rows`, isError: false };
  }
  return null;
}

export function TimelineSpanRow({
  span,
  spans,
  totalDuration,
  selectedSpanId,
  onSelectSpan,
  selfTimes,
  criticalPath,
  ancestorHasMoreSiblings = [],
  isLastChild = true
}: TimelineSpanRowProps) {
  const [expanded, setExpanded] = useState(span.depth < 3);
  const children = spans.filter((s) => s.parentSpanId === span.spanId);
  const hasChildren = children.length > 0;
  const isSelected = selectedSpanId === span.spanId;

  const offset = (span.offsetMs / totalDuration) * 100;
  const width = (span.durationMs / totalDuration) * 100;

  const category = getSpanCategory(span);
  const { barClass } = SPAN_CATEGORY_META[category];
  const isError = isErrorStatus(span.statusCode);
  const isCritical = criticalPath.has(span.spanId);

  const self = selfTimes.get(span.spanId) ?? span.durationMs;
  const childrenFraction =
    span.durationMs > 0 ? Math.max(0, (span.durationMs - self) / span.durationMs) : 0;

  const meta = spanMeta(span);

  // Build tree connector lines for ancestors
  const treeConnectors = ancestorHasMoreSiblings.map((hasMore, index) => (
    <div key={index} className='relative h-full w-5 flex-shrink-0'>
      {hasMore && <div className='absolute top-0 bottom-0 left-2.5 w-px bg-border' />}
    </div>
  ));

  return (
    <>
      <div
        className={cn(
          "group flex cursor-pointer items-center px-2 py-1.5 transition-colors",
          isSelected
            ? "border-l-2 border-l-primary bg-primary/15"
            : "border-l-2 border-l-transparent hover:bg-accent/50"
        )}
        onClick={() => {
          onSelectSpan(span);
        }}
      >
        {/* Tree structure with connectors */}
        <div className='flex h-6 flex-shrink-0 items-center'>
          {treeConnectors}

          {span.depth > 0 && (
            <div className='relative h-full w-5 flex-shrink-0'>
              <div
                className={cn(
                  "absolute top-0 left-2.5 w-px bg-border",
                  isLastChild ? "h-1/2" : "h-full"
                )}
              />
              <div className='absolute top-1/2 left-2.5 h-px w-2.5 bg-border' />
            </div>
          )}

          <div className='relative flex h-5 w-5 flex-shrink-0 items-center justify-center'>
            {hasChildren ? (
              <button
                type='button'
                onClick={(e) => {
                  e.stopPropagation();
                  setExpanded(!expanded);
                }}
                className='z-10 flex h-4 w-4 items-center justify-center border border-border bg-background transition-colors hover:bg-muted'
              >
                {expanded ? (
                  <ChevronDown className='h-3 w-3 text-muted-foreground' />
                ) : (
                  <ChevronRight className='h-3 w-3 text-muted-foreground' />
                )}
              </button>
            ) : (
              <div className='h-1.5 w-1.5 rounded-full bg-border' />
            )}
          </div>
        </div>

        {/* Span info */}
        <div className='ml-1 flex w-64 flex-shrink-0 items-center gap-2'>
          <span className={cn("h-2 w-2 flex-shrink-0 ", barClass)} />
          <SpanIcon
            spanName={span.spanName}
            className='h-4 w-4 flex-shrink-0 text-muted-foreground'
          />
          <span className='truncate font-medium text-sm' title={span.spanName}>
            {formatSpanLabel(span.spanName)}
          </span>
          {meta && (
            <span
              className={cn(
                "flex-shrink-0 truncate font-mono text-[10px] tabular-nums",
                meta.isError ? "text-destructive" : "text-muted-foreground"
              )}
              title={meta.label}
            >
              {meta.label}
            </span>
          )}
          {isError && <AlertCircle className='h-3.5 w-3.5 flex-shrink-0 text-destructive' />}
        </div>

        {/* Timeline bar */}
        <div className='relative h-5 min-w-[200px] flex-1 overflow-hidden bg-muted/30'>
          {/* Grid lines */}
          <div className='pointer-events-none absolute inset-0 flex'>
            <div className='flex-1 border-border/20 border-r' />
            <div className='flex-1 border-border/20 border-r' />
            <div className='flex-1 border-border/20 border-r' />
            <div className='flex-1' />
          </div>
          {/* Span bar */}
          <div
            className={cn(
              "absolute top-0.5 bottom-0.5 transition-all",
              barClass,
              isError && "ring-1 ring-destructive",
              isCritical && "outline outline-dashed outline-1 outline-primary/70 outline-offset-1"
            )}
            style={{
              left: `${Math.max(0, offset)}%`,
              width: `${Math.max(width, 0.5)}%`,
              minWidth: "3px"
            }}
            title={`${span.spanName}: ${formatDuration(span.durationMs)} · self ${formatDuration(self)}`}
          >
            {/* Self-time cap: the lighter segment marks time spent in children. */}
            {childrenFraction > 0.02 && (
              <div
                className='absolute inset-y-0 right-0 rounded-r-sm bg-background/45'
                style={{ width: `${childrenFraction * 100}%` }}
              />
            )}
            {width > 12 && (
              <span className='absolute inset-0 z-10 flex items-center truncate px-1.5 font-medium text-[10px] text-white'>
                {formatDuration(span.durationMs)}
              </span>
            )}
          </div>
        </div>

        {/* Duration column */}
        <div className='w-20 pr-2 text-right font-medium font-mono text-muted-foreground text-xs tabular-nums'>
          {formatDuration(span.durationMs)}
        </div>
      </div>

      {/* Children */}
      {expanded &&
        hasChildren &&
        children.map((child, index) => {
          const isLast = index === children.length - 1;
          const newAncestorHasMoreSiblings = [...ancestorHasMoreSiblings, !isLastChild];

          return (
            <TimelineSpanRow
              key={child.spanId}
              span={child}
              spans={spans}
              totalDuration={totalDuration}
              selectedSpanId={selectedSpanId}
              onSelectSpan={onSelectSpan}
              selfTimes={selfTimes}
              criticalPath={criticalPath}
              ancestorHasMoreSiblings={newAncestorHasMoreSiblings}
              isLastChild={isLast}
            />
          );
        })}
    </>
  );
}
