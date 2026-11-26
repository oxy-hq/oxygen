import { ArrowRight, Table as TableIcon } from "lucide-react";
import type React from "react";
import { cn } from "@/libs/shadcn/utils";
import type { EltTableNode, EltTableStatus, PhaseSpan } from "./model";

const STATUS_BORDER: Record<EltTableStatus, string> = {
  loaded: "border-dot-success",
  failed: "border-destructive",
  loading: "border-primary",
  normalizing: "border-dot-info",
  extracting: "border-dot-info",
  extracted: "border-dot-info",
  pending: "border-border border-dashed"
};

const STATUS_DOT: Record<EltTableStatus, string> = {
  loaded: "bg-dot-success",
  failed: "bg-destructive",
  loading: "bg-primary animate-pulse",
  normalizing: "bg-dot-info animate-pulse",
  extracting: "bg-dot-info animate-pulse",
  extracted: "bg-dot-info",
  pending: "bg-muted-foreground/30"
};

/** Compact row count: "1.2k" / "847k" / "12M". */
const formatRows = (n: number | null): string => {
  if (n === null) return "—";
  if (n < 1000) return n.toLocaleString();
  if (n < 1_000_000) return `${(n / 1000).toFixed(n < 10_000 ? 1 : 0)}k`;
  return `${(n / 1_000_000).toFixed(n < 10_000_000 ? 2 : 1)}M`;
};

/**
 * Per-table card with a tri-banded extract → normalize → load bar
 * positioned on the run's shared time axis. The banding is the hero
 * encoding — operators read "what phase ate the time" without
 * leaving the page. Drop% is called out prominently when the load
 * count is lower than the extract count.
 */
export const EltTableRow: React.FC<{
  table: EltTableNode;
  window: { t0Ms: number; t1Ms: number; spanMs: number } | null;
  isSelected: boolean;
  hasSchemaChange: boolean;
  onClick: () => void;
}> = ({ table, window, isSelected, hasSchemaChange, onClick }) => {
  const borderTone = STATUS_BORDER[table.status];
  const dot = STATUS_DOT[table.status];

  return (
    <button
      type='button'
      onClick={onClick}
      data-testid='elt-table-row'
      data-table-name={table.name}
      className={cn(
        "group w-full border-2 bg-card px-3 py-2 text-left transition-colors",
        borderTone,
        isSelected && "shadow-md ring-2 ring-ring"
      )}
    >
      <div className='flex items-center gap-2'>
        <span className={cn("h-2.5 w-2.5 shrink-0 rounded-full", dot)} />
        <TableIcon className='h-4 w-4 shrink-0 text-muted-foreground' />
        <span className='min-w-0 flex-1 truncate font-medium font-mono text-sm'>{table.name}</span>
        {hasSchemaChange && (
          <span
            className='bg-primary/15 px-1.5 py-0.5 text-primary text-xs'
            title='Schema evolved during this run — click to inspect'
          >
            schema +
          </span>
        )}
        <span className='flex items-center gap-1 text-xs tabular-nums'>
          <span className='text-muted-foreground'>{formatRows(table.rowsExtracted)}</span>
          <ArrowRight className='h-3 w-3 text-muted-foreground' />
          <span
            className={cn(
              table.dropPct !== null && table.dropPct >= 1 ? "text-warning" : "text-foreground"
            )}
          >
            {formatRows(table.rowsLoaded)}
          </span>
          {table.dropPct !== null && table.dropPct >= 1 && (
            <span className='text-warning'>· {table.dropPct}% dropped</span>
          )}
        </span>
      </div>

      {/* Tri-banded proportional bar. Each phase paints its share of
          the shared axis; gaps between bands are "waiting" time. The
          track is `relative` so the per-phase absolute bands stack
          inside it without leaking up to the card. */}
      {window && (
        <div className='relative mt-1.5 ml-5 h-2 bg-muted/40'>
          <PhaseBand phase={table.extract} window={window} tone='bg-dot-info/70' />
          <PhaseBand phase={table.normalize} window={window} tone='bg-chart-2/70' />
          <PhaseBand
            phase={table.load}
            window={window}
            tone='bg-dot-success/70'
            failed={table.status === "failed"}
          />
        </div>
      )}

      <div className='mt-1 flex items-center gap-2 pl-5 text-muted-foreground text-xs'>
        {/* All three labels are guarded — pending rows (declared in the
            plan but not yet started) have `extract === null`; the old
            unconditional render here was the source of the
            "Cannot read properties of null (reading 'completedAtMs')"
            crash on first paint. */}
        {table.extract && (
          <PhaseLabel name='extract' phase={table.extract} tone='text-status-info' />
        )}
        {table.normalize && (
          <PhaseLabel name='normalize' phase={table.normalize} tone='text-chart-2' />
        )}
        {table.load && (
          <PhaseLabel
            name='load'
            phase={table.load}
            tone={table.status === "failed" ? "text-destructive" : "text-status-success"}
          />
        )}
        {table.childTables.length > 0 && (
          <span className='tabular-nums'>· +{table.childTables.length} child tables</span>
        )}
        {table.error && <span className='truncate text-destructive italic'>· {table.error}</span>}
      </div>
    </button>
  );
};

const PhaseBand: React.FC<{
  phase: PhaseSpan | null;
  window: { t0Ms: number; spanMs: number };
  tone: string;
  failed?: boolean;
}> = ({ phase, window, tone, failed }) => {
  if (!phase || window.spanMs <= 0) return null;
  const endMs = phase.completedAtMs ?? phase.startedAtMs;
  const leftPct = Math.max(0, ((phase.startedAtMs - window.t0Ms) / window.spanMs) * 100);
  const widthPct = Math.max(((endMs - phase.startedAtMs) / window.spanMs) * 100, 0.4);
  // Bands sit absolutely-positioned within the shared track. Multiple
  // bands per row can overlap visually in pathological cases (a
  // connector that starts loading before extract closes — shouldn't
  // happen but isn't enforced) — the colour banding still reads
  // because of the distinct tones.
  return (
    <div
      className={cn(
        "absolute inset-y-0 ",
        failed ? "bg-destructive/70" : tone,
        phase.completedAtMs === null && "animate-pulse"
      )}
      style={{
        left: `${leftPct}%`,
        width: `${widthPct}%`
      }}
    />
  );
};

const PhaseLabel: React.FC<{
  name: string;
  phase: PhaseSpan;
  tone: string;
}> = ({ name, phase, tone }) => {
  const durationMs = phase.completedAtMs !== null ? phase.completedAtMs - phase.startedAtMs : null;
  return (
    <span className={cn("flex items-center gap-1", tone)}>
      <span className='font-medium'>{name}</span>
      {durationMs !== null && (
        <span className='text-muted-foreground tabular-nums'>
          {durationMs >= 1000 ? `${(durationMs / 1000).toFixed(1)}s` : `${durationMs}ms`}
        </span>
      )}
    </span>
  );
};
