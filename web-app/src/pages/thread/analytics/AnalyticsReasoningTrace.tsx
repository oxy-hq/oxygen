import { ChevronDown, ChevronLeft, ChevronRight, Layers } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Spinner } from "@/components/ui/shadcn/spinner";
import {
  type AnalyticsStep,
  buildAnalyticsSteps,
  type FanOutGroup,
  type SelectableItem,
  type StepOrGroup
} from "@/hooks/analyticsSteps";
import { cn } from "@/libs/shadcn/utils";
import type { UiBlock } from "@/services/api/analytics";
import AnalyticsStepRow from "./AnalyticsStepRow";
import useAutoCollapse from "./useAutoCollapse";

// ── Fan-out group row ─────────────────────────────────────────────────────────

interface FanOutGroupRowProps {
  group: FanOutGroup;
  onSelectArtifact: (item: SelectableItem) => void;
}

const FanOutGroupRow = ({ group, onSelectArtifact }: FanOutGroupRowProps) => {
  const [activeIndex, setActiveIndex] = useState(0);
  const safeIndex = Math.min(activeIndex, Math.max(0, group.cards.length - 1));
  const activeCard = group.cards[safeIndex];

  return (
    <div className='rounded-md border border-border'>
      {/* Card navigation header */}
      <div className='flex items-center gap-2 border-border border-b px-3 py-1.5'>
        <Layers className='h-3 w-3 shrink-0 text-muted-foreground' />
        <span className='min-w-0 flex-1 truncate text-muted-foreground text-sm'>
          {group.total} parallel queries
        </span>
        {group.isStreaming && <Spinner className='size-3 shrink-0 text-primary' />}

        <div className='flex shrink-0 items-center gap-1'>
          <button
            type='button'
            onClick={() => setActiveIndex((i) => Math.max(0, i - 1))}
            disabled={safeIndex === 0}
            className='rounded p-0.5 text-muted-foreground transition-colors hover:text-foreground disabled:opacity-30'
            aria-label='Previous query'
          >
            <ChevronLeft className='h-3.5 w-3.5' />
          </button>
          <span className='min-w-[3rem] text-center font-mono text-muted-foreground text-xs'>
            {safeIndex + 1} / {group.total}
          </span>
          <button
            type='button'
            onClick={() => setActiveIndex((i) => Math.min(group.cards.length - 1, i + 1))}
            disabled={safeIndex >= group.cards.length - 1}
            className='rounded p-0.5 text-muted-foreground transition-colors hover:text-foreground disabled:opacity-30'
            aria-label='Next query'
          >
            <ChevronRight className='h-3.5 w-3.5' />
          </button>
        </div>
      </div>

      {/* Active card content */}
      <div className='p-2'>
        {activeCard ? (
          activeCard.steps.length > 0 ? (
            <div className='space-y-1'>
              {activeCard.steps.map((step) => (
                <AnalyticsStepRow key={step.id} step={step} onSelectArtifact={onSelectArtifact} />
              ))}
            </div>
          ) : (
            <p className='py-2 text-center text-muted-foreground text-xs'>Running…</p>
          )
        ) : (
          <p className='py-2 text-center text-muted-foreground text-xs'>Waiting for results…</p>
        )}
      </div>

      {/* Dot navigation */}
      {group.total > 1 && (
        <div className='flex justify-center gap-1.5 pb-2'>
          {Array.from({ length: group.total }).map((_, i) => (
            <button
              // biome-ignore lint/suspicious/noArrayIndexKey: index is stable for fixed-count dots
              key={i}
              type='button'
              onClick={() => setActiveIndex(i)}
              aria-label={`Query ${i + 1}`}
              className={cn(
                "h-1.5 rounded-full transition-all",
                i === safeIndex
                  ? "w-4 bg-primary"
                  : "w-1.5 bg-muted-foreground/30 hover:bg-muted-foreground/60"
              )}
            />
          ))}
        </div>
      )}
    </div>
  );
};

// ── Header ────────────────────────────────────────────────────────────────────

function countSteps(items: StepOrGroup[]): { total: number; done: number } {
  let total = 0;
  let done = 0;
  for (const item of items) {
    if (item.kind === "step") {
      total++;
      if (!item.isStreaming) done++;
    } else {
      // fan_out counts as one logical unit
      total++;
      if (!item.isStreaming) done++;
    }
  }
  return { total, done };
}

function aggregateLlmStats(events: UiBlock[]): { calls: number; totalMs: number } {
  let calls = 0;
  let totalMs = 0;
  for (const ev of events) {
    if (ev.event_type === "llm_usage") {
      calls++;
      totalMs += ev.payload.duration_ms || 0;
    }
  }
  return { calls, totalMs };
}

interface HeaderProps {
  items: StepOrGroup[];
  isStreaming: boolean;
  collapsed: boolean;
  onToggle: () => void;
  llmCalls: number;
  llmTotalMs: number;
}

function formatDuration(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  return `${(ms / 1000).toFixed(1)}s`;
}

const TraceHeader = ({
  items,
  isStreaming,
  collapsed,
  onToggle,
  llmCalls,
  llmTotalMs
}: HeaderProps) => {
  const { total } = countSteps(items);
  const isComplete = !isStreaming;

  return (
    <button
      type='button'
      onClick={onToggle}
      className='flex w-full flex-wrap items-center gap-x-2 gap-y-1'
    >
      <ChevronDown
        className={cn(
          "h-3 w-3 shrink-0 text-muted-foreground transition-transform",
          collapsed && "-rotate-90"
        )}
      />
      {!isComplete && <Spinner className='size-3 shrink-0 text-primary' />}
      <span className='shrink-0 font-medium text-muted-foreground text-sm'>Reasoning trace</span>
      <span className='ml-auto flex min-w-0 flex-wrap items-center justify-end gap-x-2 font-mono text-muted-foreground text-xs'>
        {llmCalls > 0 && (
          <>
            <span className='truncate'>
              {llmCalls} LLM {llmCalls === 1 ? "call" : "calls"} · {formatDuration(llmTotalMs)}
            </span>
            <span aria-hidden='true'>·</span>
          </>
        )}
        <span className='truncate'>
          {isComplete ? `${total} steps` : total > 0 ? `${total} steps` : ""}
        </span>
      </span>
    </button>
  );
};

// ── Root ──────────────────────────────────────────────────────────────────────

interface AnalyticsReasoningTraceProps {
  events: UiBlock[];
  isRunning: boolean;
  onSelectArtifact: (item: SelectableItem) => void;
  defaultCollapsed?: boolean;
  /** Override the displayed duration (wall-clock ms) instead of summing individual LLM durations.
   *  Useful when events are aggregated from multiple parallel runs. */
  wallClockMs?: number;
  /** When true, drop the outer bordered card so the trace blends into its container. */
  flat?: boolean;
}

const AnalyticsReasoningTrace = ({
  events,
  isRunning,
  onSelectArtifact,
  defaultCollapsed = false,
  wallClockMs,
  flat = false
}: AnalyticsReasoningTraceProps) => {
  const items = buildAnalyticsSteps(events);
  const { calls: llmCalls, totalMs: llmTotalMs } = useMemo(
    () => aggregateLlmStats(events),
    [events]
  );

  const hasContent = items.length > 0;
  const [collapsed, setCollapsed] = useAutoCollapse(isRunning, hasContent, defaultCollapsed);
  const toggleCollapse = useCallback(() => setCollapsed((prev) => !prev), [setCollapsed]);

  const scrollRef = useRef<HTMLDivElement>(null);
  const bottomRef = useRef<HTMLDivElement>(null);

  // Auto-scroll to bottom on every new SSE event so content is always visible while streaming
  // biome-ignore lint/correctness/useExhaustiveDependencies: scroll on every event/collapse change
  useEffect(() => {
    if (!collapsed && scrollRef.current) {
      scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
    }
  }, [events.length, collapsed]);

  if (!isRunning && !hasContent) return null;

  return (
    <div className={cn(!flat && "rounded-lg border border-border p-3")}>
      <TraceHeader
        items={items}
        isStreaming={isRunning}
        collapsed={collapsed}
        onToggle={toggleCollapse}
        llmCalls={llmCalls}
        llmTotalMs={wallClockMs ?? llmTotalMs}
      />

      <div
        ref={scrollRef}
        className={cn(
          "transition-all duration-500",
          collapsed
            ? "max-h-0 overflow-hidden opacity-0"
            : cn("mt-1.5 overflow-y-auto opacity-100", flat ? "max-h-[180px]" : "max-h-[600px]")
        )}
      >
        <div className='space-y-1.5'>
          {items.map((item) =>
            item.kind === "fan_out" ? (
              <FanOutGroupRow key={item.id} group={item} onSelectArtifact={onSelectArtifact} />
            ) : (
              <AnalyticsStepRow
                key={item.id}
                step={item as AnalyticsStep}
                onSelectArtifact={onSelectArtifact}
                flat={flat}
              />
            )
          )}
        </div>
        <div ref={bottomRef} />
      </div>
    </div>
  );
};

export default AnalyticsReasoningTrace;
