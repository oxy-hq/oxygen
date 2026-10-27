import { useState } from "react";
import { toast } from "sonner";
import ErrorAlert from "@/components/ui/ErrorAlert";
import { Button } from "@/components/ui/shadcn/button";
import { useExperimentDesign, useRetrospectiveExperiment } from "@/hooks/api/useExperiments";
import { useRole } from "@/hooks/useRole";
import { ToneBadge } from "@/pages/ide/Experiments/components/glance/tone";
import type { ExperimentListItem } from "@/types/experiment";

/**
 * Past-dated design: offer to analyse it from existing data. The result is
 * labelled "not pre-registered"; no lever is switched. Owner/Admin only.
 */
export default function RetrospectiveOffer({ summary }: { summary: ExperimentListItem }) {
  const designQuery = useExperimentDesign(summary.name);
  const retrospective = useRetrospectiveExperiment(summary.name);
  const { is } = useRole();
  const [error, setError] = useState<string | null>(null);

  const design = designQuery.data;

  const run = async () => {
    if (!design) return;
    setError(null);
    try {
      await retrospective.mutateAsync(design.content_hash);
      toast.success("Analysing past data for this design");
    } catch (e) {
      setError(e instanceof Error ? e.message : "The analysis could not be started.");
    }
  };

  return (
    <section
      className='flex flex-col gap-3 rounded-lg border bg-card p-4'
      data-testid='experiment-retrospective-offer'
    >
      <div className='flex flex-wrap items-center justify-between gap-2'>
        <h3 className='font-medium text-sm'>This design can be analysed from past data</h3>
        <ToneBadge tone='info'>Retrospective available</ToneBadge>
      </div>
      <p className='text-muted-foreground text-xs'>
        All of this design's dates are in the past. The result will be labelled "not
        pre-registered": it is exploratory, not a registered test, and no levers are switched.
      </p>
      <div className='flex flex-wrap items-center gap-3'>
        <Button
          disabled={!design || !is.workspaceAdmin || retrospective.isPending}
          title={
            is.workspaceAdmin
              ? undefined
              : "Only workspace owners and admins can analyse past data."
          }
          onClick={run}
          data-testid='experiment-retrospective'
        >
          Analyse past data
        </Button>
        {!is.workspaceAdmin && (
          <span className='text-muted-foreground text-xs'>
            Only workspace owners and admins can analyse past data.
          </span>
        )}
      </div>
      {error && (
        <div data-testid='experiment-retrospective-error'>
          <ErrorAlert title='The analysis was not started' message={error} />
        </div>
      )}
    </section>
  );
}
