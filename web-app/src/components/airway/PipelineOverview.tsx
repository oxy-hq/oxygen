/**
 * Pipeline Overview — the always-meaningful landing surface
 * (Dagster/Airbyte shape): a short description, a recent-runs health
 * strip (Airflow/Dagster style), and the lineage of the most recent
 * run. No new backend: the strip uses the runs list and the lineage
 * replays the latest run through the existing stream + reducer.
 */

import type React from "react";
import LineageGraph from "@/components/airway/LineageGraph";
import ResetCursorsButton from "@/components/airway/ResetCursorsButton";
import ResetSchemaButton from "@/components/airway/ResetSchemaButton";
import { useAirwayRunStream, useAirwayRuns } from "@/hooks/api/airway/useAirway";
import { cn } from "@/libs/shadcn/utils";
import type { AirwayRunSummary } from "@/services/api/airway";

/** Status → strip-cell background (semantic tokens only). */
function cellClass(status: string): string {
  switch (status) {
    case "done":
      return "bg-primary";
    case "failed":
      return "bg-destructive";
    case "completed_with_errors":
      return "bg-primary/40";
    case "cancelled":
      return "bg-muted-foreground/40";
    case "running":
      return "bg-primary/60 animate-pulse";
    default:
      return "bg-muted-foreground/30";
  }
}

const fmt = (iso: string) => {
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleString();
};

/** Most-recent-N runs as clickable status bars, oldest → newest. */
const RecentRunsStrip: React.FC<{
  runs: AirwayRunSummary[];
  onOpenRun: (runId: string) => void;
}> = ({ runs, onOpenRun }) => {
  // `useAirwayRuns` returns newest-first; show oldest→newest so the
  // latest run is on the right (Airflow/Dagster convention).
  const ordered = [...runs].slice(0, 30).reverse();
  return (
    <div className='flex items-end gap-1'>
      {ordered.map((r) => (
        <button
          key={r.run_id}
          type='button'
          onClick={() => onOpenRun(r.run_id)}
          title={`${r.run_id.slice(0, 8)} · ${r.status} · ${fmt(r.created_at)}`}
          aria-label={`Open run ${r.run_id.slice(0, 8)} (${r.status})`}
          className={cn(
            "h-8 w-3 rounded-sm transition-opacity hover:opacity-70",
            cellClass(r.status)
          )}
        />
      ))}
    </div>
  );
};

const PipelineOverview: React.FC<{
  pipelineRef: string;
  onOpenRun: (runId: string) => void;
}> = ({ pipelineRef, onOpenRun }) => {
  const { data: runs, isLoading, isError } = useAirwayRuns(pipelineRef);
  const latestRunId = runs?.[0]?.run_id;
  // Replays the latest run (terminal runs replay then close) so the
  // overview shows a representative lineage without a run-config API.
  const { view } = useAirwayRunStream(latestRunId);

  return (
    <div className='flex flex-col gap-6 p-4'>
      <div className='flex items-start justify-between gap-4'>
        <div>
          <h2 className='font-medium text-sm'>{pipelineRef}</h2>
          <p className='mt-0.5 text-muted-foreground text-xs'>
            ELT pipeline · {runs?.length ?? 0} run{runs?.length === 1 ? "" : "s"}
          </p>
        </div>
        {/* Order is the safety property, not the styling. The rewind keeps
            every row and comes first carrying the outline; the schema reset
            drops every destination table and reads as secondary. Before the
            rewind existed, the destructive one was the only reset on screen,
            so it was also the one reached for to do a rewind's job. */}
        <div className='flex items-center gap-1'>
          <ResetCursorsButton pipelineRef={pipelineRef} />
          <ResetSchemaButton pipelineRef={pipelineRef} />
        </div>
      </div>

      <section>
        <h3 className='mb-2 font-medium text-muted-foreground text-xs uppercase tracking-wide'>
          Recent runs
        </h3>
        {isLoading ? (
          <div className='text-muted-foreground text-xs'>Loading runs…</div>
        ) : isError ? (
          <div className='text-destructive text-xs'>Couldn’t load runs.</div>
        ) : !runs || runs.length === 0 ? (
          <div className='text-muted-foreground text-xs'>
            No runs yet — press Run to start this pipeline.
          </div>
        ) : (
          <RecentRunsStrip runs={runs} onOpenRun={onOpenRun} />
        )}
      </section>

      <section className='min-h-0 flex-1'>
        <h3 className='mb-2 font-medium text-muted-foreground text-xs uppercase tracking-wide'>
          Lineage{latestRunId ? " (latest run)" : ""}
        </h3>
        {latestRunId ? (
          <LineageGraph view={view} />
        ) : (
          <div className='rounded-md border border-border border-dashed px-4 py-10 text-center text-muted-foreground text-sm'>
            Run the pipeline to see how data moves from sources to the destination.
          </div>
        )}
      </section>
    </div>
  );
};

export default PipelineOverview;
