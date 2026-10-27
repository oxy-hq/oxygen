import { useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import { useExperimentRun } from "@/hooks/api/useExperiments";
import { useRole } from "@/hooks/useRole";
import { cn } from "@/libs/shadcn/utils";
import { EmptyState, ErrorState, LoadingState } from "@/pages/ide/coordinator/components/PageState";
import { COLUMN } from "@/pages/ide/Experiments/ExperimentDetail/column";
import type { ExperimentListItem } from "@/types/experiment";
import AbortDialog from "./components/AbortDialog";
import RunBody from "./components/RunBody";

/**
 * Registered / live / closing: operations only. Mounted for those three
 * statuses and no others, so the run route is never asked about a draft and the
 * result route is never reached from here at all.
 */
export default function OperationsView({ summary }: { summary: ExperimentListItem }) {
  const { data, isPending, error, refetch } = useExperimentRun(summary.name);
  const [abortOpen, setAbortOpen] = useState(false);
  const { is } = useRole();

  if (isPending) return <LoadingState />;
  if (error) return <ErrorState message={error.message} onRetry={() => refetch()} />;
  const run = data?.run;
  if (!run) return <EmptyState title='No run is recorded for this experiment yet' />;

  // The server refuses an abort once the run is closing (409), so it is not offered.
  // An accepted abort stays live until the next tick; a second one would keep the first reason.
  if (run.retrospective) {
    return (
      <div className={COLUMN} data-testid='experiment-operations'>
        <p className='text-muted-foreground text-sm' data-testid='experiment-retrospective-note'>
          Analysing past data — no levers are switched. The report is being prepared.
        </p>
      </div>
    );
  }

  const abortRequested = run.abort_reason !== null;
  const abortable = !abortRequested && (run.state === "registered" || run.state === "live");

  return (
    <div className={cn(COLUMN, "flex flex-col gap-4")} data-testid='experiment-operations'>
      <div className='flex items-start justify-between gap-3'>
        <p className='text-muted-foreground text-sm'>
          The registered result is sealed until the run reports; the interim below is a peek, not
          the result.
        </p>
        {abortable && !is.workspaceAdmin ? (
          <p
            className='shrink-0 text-muted-foreground text-xs'
            data-testid='experiment-abort-denied'
          >
            Only a workspace owner or admin can abort an experiment.
          </p>
        ) : abortable ? (
          <Button
            variant='destructive'
            size='sm'
            onClick={() => setAbortOpen(true)}
            data-testid='experiment-abort-button'
          >
            Abort experiment
          </Button>
        ) : abortRequested ? (
          <p
            className='shrink-0 text-muted-foreground text-xs'
            data-testid='experiment-abort-requested'
          >
            Abort requested: {run.abort_reason}
            {run.abort_levers === "keep" && " · Levers will be left as they are"}
          </p>
        ) : (
          <p className='shrink-0 text-muted-foreground text-xs'>
            A closing run can no longer be aborted.
          </p>
        )}
      </div>
      <RunBody name={summary.name} run={run} leverRevisions={data?.lever_revisions ?? []} />
      <AbortDialog
        open={abortOpen}
        onOpenChange={setAbortOpen}
        name={summary.name}
        owner={summary.owner}
        unitsOn={run.units_on}
      />
    </div>
  );
}
