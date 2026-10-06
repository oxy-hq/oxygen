import { useCallback, useMemo, useState } from "react";
import { useParams } from "react-router-dom";
import {
  ResizableHandle,
  ResizablePanel,
  ResizablePanelGroup
} from "@/components/ui/shadcn/resizable";
import useTraceDetail from "@/hooks/api/traces/useTraceDetail";
import type { TimelineSpan } from "@/services/api/traces";
import { ErrorPage } from "../components/ErrorPage";
import { SpanDetailPanel } from "./components/SpanDetailPanel";
import { computeTraceSpanMetrics, criticalSelfPercent } from "./components/spanMetrics";
import { Timeline } from "./components/Timeline";
import { TraceHeader } from "./components/TraceHeader";
import { TraceSummaryStrip } from "./components/TraceSummaryStrip";
import { summarizeTrace } from "./components/traceSummary";

export default function TraceDetailPage() {
  const { traceId } = useParams<{ traceId: string }>();
  const { data: trace, isLoading, error } = useTraceDetail(traceId || "");
  const [selectedSpanId, setSelectedSpanId] = useState<string | null>(null);

  // Only show selected span when explicitly clicked (no auto-select)
  const selectedSpan = trace?.spans.find((s) => s.spanId === selectedSpanId) ?? null;

  // Self-time per span + critical path — powers the waterfall's self-time cap,
  // critical-path highlight, and the inspector's self-vs-children split.
  const metrics = useMemo(() => computeTraceSpanMetrics(trace?.spans ?? []), [trace?.spans]);
  const summary = useMemo(() => summarizeTrace(trace?.spans ?? []), [trace?.spans]);

  const handleSelectSpan = useCallback((span: TimelineSpan) => {
    setSelectedSpanId(span.spanId);
  }, []);

  const handleClosePanel = useCallback(() => {
    setSelectedSpanId(null);
  }, []);

  if (isLoading) {
    return (
      <div className='flex h-full items-center justify-center'>
        <div className='text-muted-foreground'>Loading trace...</div>
      </div>
    );
  }

  if (error || !trace) {
    return (
      <ErrorPage message='Failed to load trace' description={error?.message || "Trace not found"} />
    );
  }

  return (
    <div className='flex h-full flex-col bg-background'>
      <TraceHeader
        traceId={traceId || ""}
        totalDurationMs={trace.totalDurationMs}
        spansCount={trace.spans.length}
        startTime={trace.startTime}
      />

      <div className='border-b px-4 py-3'>
        <TraceSummaryStrip
          summary={summary}
          totalDurationMs={trace.totalDurationMs}
          criticalPercent={criticalSelfPercent(metrics, trace.totalDurationMs)}
        />
      </div>

      {/* Main Content with Resizable Panels */}
      <div className='flex-1 overflow-hidden'>
        <ResizablePanelGroup direction='horizontal'>
          {/* Timeline Panel */}
          <ResizablePanel
            defaultSize={selectedSpan ? 50 : 100}
            minSize={30}
            className='flex flex-col'
          >
            <div className='scrollbar-gutter-auto flex-1 overflow-auto'>
              <Timeline
                spans={trace.spans}
                totalDuration={trace.totalDurationMs}
                selectedSpanId={selectedSpan?.spanId}
                onSelectSpan={handleSelectSpan}
                selfTimes={metrics.selfTimes}
                criticalPath={metrics.criticalPath}
              />
            </div>
          </ResizablePanel>

          {/* Detail Panel - Only shown when span is selected */}
          {selectedSpan && (
            <>
              <ResizableHandle withHandle />
              <ResizablePanel defaultSize={50} minSize={25} className='flex flex-col'>
                <SpanDetailPanel
                  key={selectedSpan.spanId}
                  span={selectedSpan}
                  selfMs={metrics.selfTimes.get(selectedSpan.spanId) ?? 0}
                  onClose={handleClosePanel}
                />
              </ResizablePanel>
            </>
          )}
        </ResizablePanelGroup>
      </div>
    </div>
  );
}
