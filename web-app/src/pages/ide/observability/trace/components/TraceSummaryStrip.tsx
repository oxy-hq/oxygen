import { AlertCircle } from "lucide-react";
import type { ReactNode } from "react";
import { cn } from "@/libs/shadcn/utils";
import { formatDuration } from "../../utils/index";
import type { TraceSummary } from "./traceSummary";

interface TraceSummaryStripProps {
  summary: TraceSummary;
  totalDurationMs: number;
  /** Share of wall time spent on the critical path (0–100), if computable. */
  criticalPercent?: number;
}

function formatCompact(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return n.toString();
}

interface StatTileProps {
  label: string;
  value: ReactNode;
  unit?: string;
  tone?: "default" | "error";
  icon?: ReactNode;
  title?: string;
}

function StatTile({ label, value, unit, tone = "default", icon, title }: StatTileProps) {
  return (
    <div className='flex flex-col gap-1 bg-card px-4 py-3' title={title}>
      <span className='t-label text-muted-foreground'>{label}</span>
      <span
        className={cn(
          "t-h2 flex items-center gap-1 tabular-nums",
          tone === "error" && "text-destructive"
        )}
      >
        {icon}
        {value}
        {unit && <span className='t-small text-muted-foreground'>{unit}</span>}
      </span>
    </div>
  );
}

export function TraceSummaryStrip({
  summary,
  totalDurationMs,
  criticalPercent
}: TraceSummaryStripProps) {
  const hasErrors = summary.errorCount > 0;
  const wall = formatDuration(totalDurationMs);
  // No frontend model→price map, so per-trace cost can't be computed honestly.
  // The /cost endpoint owns authoritative cost; show tokens here, dash cost.

  return (
    // One ruled strip, not eight cards: the gap between cells is a hairline
    // of the border colour showing through, so the rules stay whole however
    // the grid wraps.
    <div className='grid grid-cols-2 gap-px overflow-hidden rounded-lg border bg-border sm:grid-cols-4 xl:grid-cols-8'>
      <StatTile label='Spans' value={summary.spanCount} />
      <StatTile
        label='Errors'
        value={summary.errorCount}
        tone={hasErrors ? "error" : "default"}
        icon={hasErrors ? <AlertCircle className='h-4 w-4' /> : undefined}
      />
      <StatTile label='LLM calls' value={summary.llmCallCount} />
      <StatTile label='Tool calls' value={summary.toolCallCount} />
      <StatTile label='Tokens' value={formatCompact(summary.totalTokens)} />
      <StatTile
        label='Est. cost'
        value='—'
        title='No model price map on the client — see the Cost & tokens table in Execution Analytics'
      />
      <StatTile label='Wall time' value={wall} />
      <StatTile
        label='Self / crit'
        value={criticalPercent !== undefined ? criticalPercent : "—"}
        unit={criticalPercent !== undefined ? "%" : undefined}
        title='Share of wall time spent on the critical path'
      />
    </div>
  );
}
