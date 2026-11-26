import { Brain, ChevronsRight, Code2, Database, GitBranch, Wrench } from "lucide-react";
import type React from "react";
import { useEffect, useMemo, useState } from "react";
import { cn } from "@/libs/shadcn/utils";
import type { RunEventEntry } from "@/services/api/coordinator";
import { formatTokens } from "../../../../components/utils";
import { buildWaterfall, type ChildSpan, colorsFor, formatMs, type PhaseSpan } from "./model";
import { SpanPreview } from "./SpanPreview";

/**
 * Waterfall view — phase rows on a shared time axis, with nested LLM /
 * tool / thinking spans inside each phase. The preview panel is fixed
 * on the right (sticky) so hover-induced repaint doesn't shift the
 * bars list under the cursor — that was producing flicker when the
 * user moved the cursor down a long list.
 *
 * Hover sets the focused span. Moving off a span keeps the last
 * selection visible (no `onMouseLeave` clear) so the panel stays
 * useful while the user reads or scrolls.
 */
export const Waterfall: React.FC<{ events: RunEventEntry[] }> = ({ events }) => {
  const model = useMemo(() => buildWaterfall(events), [events]);
  const [focused, setFocused] = useState<ChildSpan | null>(null);

  // Default-focus the first child span on mount so the side panel is
  // never empty on first render — better than a "hover something"
  // placeholder when the user has just opened the page.
  useEffect(() => {
    if (focused) return;
    const firstWithChildren = model.phases.find((p) => p.children.length > 0);
    if (firstWithChildren) setFocused(firstWithChildren.children[0]);
  }, [model.phases, focused]);

  if (model.phases.length === 0) {
    return (
      <div className='px-4 py-10 text-center text-muted-foreground text-sm'>
        No phase transitions captured for this run yet. Once the agent enters its first FSM state,
        bars will appear here.
      </div>
    );
  }

  const total = Math.max(model.totalMs, 1);

  return (
    <div className='flex flex-col gap-4 p-3 lg:flex-row'>
      <div className='min-w-0 flex-1 space-y-1'>
        {model.phases.map((phase) => (
          <PhaseRow
            key={`${phase.state}-${phase.index}`}
            phase={phase}
            totalMs={total}
            focusedId={focused?.id ?? null}
            onFocus={setFocused}
          />
        ))}
      </div>
      <aside className='lg:w-96 lg:shrink-0'>
        <div className='sticky top-3 max-h-[calc(100vh-8rem)] overflow-y-auto border border-border bg-card'>
          <SpanPreview span={focused} />
        </div>
      </aside>
    </div>
  );
};

const PhaseRow: React.FC<{
  phase: PhaseSpan;
  totalMs: number;
  focusedId: string | null;
  onFocus: (s: ChildSpan | null) => void;
}> = ({ phase, totalMs, focusedId, onFocus }) => {
  const colors = colorsFor(phase.state);
  const leftPct = (phase.startMs / totalMs) * 100;
  const widthPct = Math.max((phase.durationMs / totalMs) * 100, 0.5);

  return (
    <div className='group'>
      <div className='flex items-center gap-2'>
        <span className={cn("w-24 shrink-0 truncate font-medium text-xs capitalize", colors.text)}>
          {phase.state}
        </span>
        <div className='relative h-5 flex-1 bg-muted/40'>
          <div
            className={cn("absolute inset-y-0 ", colors.bg)}
            style={{ left: `${leftPct}%`, width: `${widthPct}%` }}
            title={`${phase.state} · ${formatMs(phase.durationMs)}`}
          />
        </div>
        <span className='w-44 shrink-0 truncate text-right text-muted-foreground text-xs tabular-nums'>
          {formatMs(phase.durationMs)} · {phase.llmCalls} LLM · {phase.toolCalls} tool ·{" "}
          {formatTokens(phase.totalTokens)}
        </span>
      </div>

      {phase.children.length > 0 && (
        <div className='mt-0.5 space-y-0.5'>
          {phase.children.map((child) => (
            <ChildRow
              key={child.id}
              child={child}
              totalMs={totalMs}
              focused={focusedId === child.id}
              onFocus={onFocus}
            />
          ))}
        </div>
      )}
    </div>
  );
};

const ChildRow: React.FC<{
  child: ChildSpan;
  totalMs: number;
  focused: boolean;
  onFocus: (s: ChildSpan | null) => void;
}> = ({ child, totalMs, focused, onFocus }) => {
  const leftPct = (child.startMs / totalMs) * 100;
  const widthPct = Math.max((child.durationMs / totalMs) * 100, 0.4);
  const Icon =
    child.kind === "llm"
      ? Brain
      : child.kind === "tool"
        ? Wrench
        : child.kind === "subrun"
          ? GitBranch
          : child.kind === "query"
            ? Database
            : child.kind === "step"
              ? ChevronsRight
              : Code2;
  // Bar tone is monochrome by design: child kind is conveyed by the
  // icon (Brain / Wrench / GitBranch / Database / etc.), so coloring
  // the bar by kind too would be rainbow-encoding the same fact.
  // Error → destructive (the one categorical color worth keeping —
  // it overrides everything below it visually). Otherwise a primary
  // tint, scaled subtly: LLM rounds are the load-bearing signal so
  // they read full-strength; tools / sub-runs / queries / steps step
  // down to /50 so they recede behind the structural rhythm.
  const tone =
    child.status === "error"
      ? "bg-destructive/70"
      : child.kind === "llm"
        ? "bg-primary/70"
        : child.kind === "thinking"
          ? "bg-muted-foreground/30"
          : "bg-primary/50";

  // Note: deliberately no `onMouseLeave` clear. Leaving the cursor over
  // empty space between rows previously dropped focus and re-set it
  // on the next row → the side panel flickered through "empty" state
  // on every row transition. Keeping last focus is both calmer and
  // more useful — the user can read the panel after moving the
  // cursor away.
  return (
    <button
      type='button'
      onMouseEnter={() => onFocus(child)}
      onFocus={() => onFocus(child)}
      onClick={() => onFocus(child)}
      className={cn(
        "flex w-full items-center gap-2 px-1 text-left transition-colors",
        focused && "bg-muted"
      )}
    >
      <span className='flex w-24 shrink-0 items-center gap-1 truncate pl-5 text-muted-foreground text-xs'>
        <Icon className='h-3 w-3 shrink-0' />
        <span className='truncate'>{child.label}</span>
      </span>
      <div className='relative h-2.5 flex-1 bg-muted/30'>
        <div
          className={cn("absolute inset-y-0 ", tone)}
          style={{ left: `${leftPct}%`, width: `${widthPct}%` }}
        />
      </div>
      <span className='w-44 shrink-0 truncate text-right text-muted-foreground text-xs tabular-nums'>
        {child.kind === "llm" && child.llm
          ? `${formatMs(child.durationMs)} · ${formatTokens(child.llm.outputTokens)} out`
          : child.kind === "tool"
            ? `${formatMs(child.durationMs)} · ${child.status === "error" ? "error" : "ok"}`
            : child.kind === "subrun" && child.subrun
              ? `${formatMs(child.durationMs)} · ${child.subrun.nested.phases.length} phase${child.subrun.nested.phases.length === 1 ? "" : "s"}`
              : child.kind === "query" && child.query
                ? `${formatMs(child.durationMs)} · ${child.query.success ? `${child.query.rowCount.toLocaleString()} rows` : "failed"}`
                : child.kind === "step"
                  ? `${formatMs(child.durationMs)} · ${child.status === "error" ? "failed" : "ok"}`
                  : formatMs(child.durationMs)}
      </span>
    </button>
  );
};
