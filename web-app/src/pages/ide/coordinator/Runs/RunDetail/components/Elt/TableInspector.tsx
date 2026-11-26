import { AlertTriangle, GitCommit, Table as TableIcon } from "lucide-react";
import type React from "react";
import { cn } from "@/libs/shadcn/utils";
import type { EltTableNode, PhaseSpan, SchemaChange } from "./model";

/** Compact ms / s formatter for phase durations. */
const formatDur = (ms: number): string =>
  ms < 1000 ? `${ms}ms` : `${(ms / 1000).toFixed(ms < 10_000 ? 1 : 0)}s`;

/** Compact row count: "1.2k" / "847k" / "12M". */
const formatRows = (n: number | null): string => {
  if (n === null) return "—";
  if (n < 1000) return n.toLocaleString();
  if (n < 1_000_000) return `${(n / 1000).toFixed(n < 10_000 ? 1 : 0)}k`;
  return `${(n / 1_000_000).toFixed(n < 10_000_000 ? 2 : 1)}M`;
};

/**
 * Side panel for a selected ELT table. Surfaces:
 *   - Per-phase durations + row counts (the "where did the time go" answer).
 *   - Drop% summary when load < extract.
 *   - Child tables produced by relational normalization (rare; airway-only).
 *   - Schema diff entries that touched *this* table.
 *   - Failure message when the table errored mid-sync.
 */
export const EltTableInspector: React.FC<{
  table: EltTableNode | null;
  schemaChanges: SchemaChange[];
}> = ({ table, schemaChanges }) => {
  if (!table) {
    return (
      <div className='flex h-32 items-center justify-center px-4 text-muted-foreground text-xs'>
        Select a table to inspect.
      </div>
    );
  }

  const myChanges = relevantSchemaChanges(schemaChanges, table.name);

  return (
    <div className='space-y-3 p-4'>
      <div className='flex items-center gap-2'>
        <TableIcon className='h-4 w-4 text-primary' />
        <span className='truncate font-mono font-semibold text-sm'>{table.name}</span>
        <span className='ml-auto text-muted-foreground text-xs uppercase tracking-wide'>
          {table.status}
        </span>
      </div>

      {table.error && (
        <div className='flex items-start gap-2 border border-destructive/40 bg-destructive/5 p-2 text-destructive text-xs'>
          <AlertTriangle className='mt-0.5 h-3.5 w-3.5 shrink-0' />
          <span className='break-words'>{table.error}</span>
        </div>
      )}

      <RowsFlow table={table} />

      <PhaseList table={table} />

      {table.childTables.length > 0 && (
        <div>
          <p className='mb-1 text-muted-foreground text-xs uppercase tracking-wide'>
            child tables (normalize)
          </p>
          <ul className='space-y-0.5 border border-border bg-muted/20 p-2 text-xs'>
            {table.childTables.map((c) => (
              <li key={c} className='truncate font-mono'>
                {c}
              </li>
            ))}
          </ul>
        </div>
      )}

      {myChanges.length > 0 && (
        <div>
          <p className='mb-1 flex items-center gap-1 text-muted-foreground text-xs uppercase tracking-wide'>
            <GitCommit className='h-3 w-3' />
            schema diff
          </p>
          <pre className='max-h-48 overflow-y-auto whitespace-pre-wrap break-words border border-border bg-muted/40 p-2 font-mono text-xs'>
            {JSON.stringify(myChanges, null, 2)}
          </pre>
        </div>
      )}
    </div>
  );
};

/** Rows-in → rows-out summary with drop% callout when applicable. */
const RowsFlow: React.FC<{ table: EltTableNode }> = ({ table }) => (
  <div className='flex items-center gap-2 border border-border bg-muted/20 p-2 text-xs'>
    <div className='flex flex-col items-center'>
      <span className='text-muted-foreground uppercase tracking-wide'>extract</span>
      <span className='font-medium tabular-nums'>{formatRows(table.rowsExtracted)}</span>
    </div>
    {table.rowsNormalized !== null && (
      <>
        <span className='text-muted-foreground'>→</span>
        <div className='flex flex-col items-center'>
          <span className='text-muted-foreground uppercase tracking-wide'>normalize</span>
          <span className='font-medium tabular-nums'>{formatRows(table.rowsNormalized)}</span>
        </div>
      </>
    )}
    <span className='text-muted-foreground'>→</span>
    <div className='flex flex-col items-center'>
      <span className='text-muted-foreground uppercase tracking-wide'>load</span>
      <span
        className={cn(
          "font-medium tabular-nums",
          table.dropPct !== null && table.dropPct >= 1 ? "text-warning" : "text-foreground"
        )}
      >
        {formatRows(table.rowsLoaded)}
      </span>
    </div>
    {table.dropPct !== null && table.dropPct >= 1 && (
      <span className='ml-auto bg-warning/15 px-1.5 py-0.5 text-warning'>
        {table.dropPct}% dropped
      </span>
    )}
  </div>
);

/** Per-phase duration list — explicit "extract: 1.2s · normalize: 0.4s ·
 *  load: 0.9s" so the slowest phase is one glance away. */
const PhaseList: React.FC<{ table: EltTableNode }> = ({ table }) => {
  const rows: Array<{ label: string; phase: PhaseSpan | null; tone: string }> = [
    { label: "extract", phase: table.extract, tone: "text-status-info" },
    { label: "normalize", phase: table.normalize, tone: "text-chart-2" },
    {
      label: "load",
      phase: table.load,
      tone: table.status === "failed" ? "text-destructive" : "text-status-success"
    }
  ].filter((r) => r.phase);

  if (rows.length === 0) return null;

  return (
    <div>
      <p className='mb-1 text-muted-foreground text-xs uppercase tracking-wide'>phases</p>
      <ul className='space-y-1'>
        {rows.map((row) => {
          const phase = row.phase as PhaseSpan;
          const durationMs =
            phase.completedAtMs !== null ? phase.completedAtMs - phase.startedAtMs : null;
          return (
            <li
              key={row.label}
              className='flex items-center justify-between gap-2 text-xs tabular-nums'
            >
              <span className={cn("font-medium capitalize", row.tone)}>{row.label}</span>
              <span className='text-muted-foreground'>
                {durationMs !== null ? formatDur(durationMs) : "running…"}
              </span>
            </li>
          );
        })}
      </ul>
    </div>
  );
};

/** Filter the schema-evolution events down to ones that mention this
 *  table. The airway payload shape isn't strict — we do a recursive
 *  string search so future shape changes don't break the filter. */
const relevantSchemaChanges = (changes: SchemaChange[], tableName: string): SchemaChange[] => {
  const needle = tableName.toLowerCase();
  return changes.filter((c) => {
    try {
      return JSON.stringify(c.changes).toLowerCase().includes(needle);
    } catch {
      return false;
    }
  });
};
