import { AlertTriangle, ArrowRight, ArrowUp, Database, GitCommit } from "lucide-react";
import type React from "react";
import { cn } from "@/libs/shadcn/utils";
import type { EltPipelineModel } from "./model";

/** Compact row count: "1.2k" / "847k" / "12M". Mirrors the helper used
 *  inside EltTablesCard. */
const formatRows = (n: number): string => {
  if (n < 1000) return n.toLocaleString();
  if (n < 1_000_000) return `${(n / 1000).toFixed(n < 10_000 ? 1 : 0)}k`;
  return `${(n / 1_000_000).toFixed(n < 10_000_000 ? 2 : 1)}M`;
};

/** Header strip for an ELT run: source connector → destination, with
 *  aggregate row counts and the schema-evolution banner stacked
 *  underneath when applicable. The lineage shape is intentionally
 *  minimal (two cards + arrow) — the per-table flow lives below. */
export const PipelineHeader: React.FC<{ model: EltPipelineModel }> = ({ model }) => {
  const { sourceLabel, destination, rollup, schemaChanges, pipelineError, cancelled } = model;

  return (
    <div className='border-border border-b bg-card'>
      <div className='flex flex-wrap items-center gap-3 px-4 py-3'>
        <Endpoint label={sourceLabel} kind='source' />
        <FlowArrow rollup={rollup} />
        <Endpoint label={destination ?? "destination"} kind='destination' />
        <div className='ml-auto flex flex-wrap items-center gap-3 text-muted-foreground text-xs'>
          <span className='tabular-nums'>{rollup.totalTables} tables</span>
          {rollup.loadedTables > 0 && (
            <span className='text-status-success'>✓ {rollup.loadedTables} loaded</span>
          )}
          {rollup.failedTables > 0 && (
            <span className='text-destructive'>✗ {rollup.failedTables} failed</span>
          )}
        </div>
      </div>

      {pipelineError && (
        <Banner tone='error' icon={AlertTriangle}>
          <span className='font-medium'>Pipeline error:</span> {pipelineError}
        </Banner>
      )}
      {cancelled && !pipelineError && (
        <Banner tone='warn' icon={AlertTriangle}>
          Run was cancelled mid-flight.
        </Banner>
      )}
      {schemaChanges.length > 0 && (
        <Banner tone='info' icon={GitCommit}>
          <span className='font-medium'>
            Schema evolved · {schemaChanges.length} change
            {schemaChanges.length === 1 ? "" : "s"}
          </span>{" "}
          — see the inspector for each table whose schema shifted.
        </Banner>
      )}
    </div>
  );
};

const Endpoint: React.FC<{ label: string; kind: "source" | "destination" }> = ({ label, kind }) => (
  <div className='flex items-center gap-2 border border-border bg-muted/40 px-3 py-1.5'>
    <Database
      className={cn("h-4 w-4 shrink-0", kind === "source" ? "text-status-success" : "text-chart-3")}
    />
    <div className='min-w-0'>
      <p className='text-muted-foreground text-xs uppercase tracking-wide'>{kind}</p>
      <p className='truncate font-medium font-mono text-sm'>{label}</p>
    </div>
  </div>
);

const FlowArrow: React.FC<{ rollup: EltPipelineModel["rollup"] }> = ({ rollup }) => (
  <div className='flex items-center gap-2 text-muted-foreground text-xs'>
    <ArrowRight className='h-4 w-4' />
    <span className='flex items-center gap-1 tabular-nums'>
      <ArrowUp className='h-3 w-3' />
      {formatRows(rollup.rowsExtracted)} in
    </span>
    <ArrowRight className='h-4 w-4' />
    <span className='flex items-center gap-1 tabular-nums'>
      {formatRows(rollup.rowsLoaded)} out
    </span>
    <ArrowRight className='h-4 w-4' />
  </div>
);

const Banner: React.FC<{
  tone: "info" | "warn" | "error";
  icon: React.ElementType;
  children: React.ReactNode;
}> = ({ tone, icon: Icon, children }) => (
  <div
    className={cn(
      "flex items-start gap-2 border-border border-t px-4 py-2 text-xs",
      tone === "error" && "bg-destructive/5 text-destructive",
      tone === "warn" && "bg-warning/10 text-warning",
      tone === "info" && "bg-primary/5 text-foreground"
    )}
  >
    <Icon className='mt-0.5 h-3.5 w-3.5 shrink-0' />
    <span className='break-words'>{children}</span>
  </div>
);
