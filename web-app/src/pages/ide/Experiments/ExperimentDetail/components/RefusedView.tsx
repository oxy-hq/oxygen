import { cn } from "@/libs/shadcn/utils";
import { StatGrid, StatTile } from "@/pages/ide/Experiments/components/glance/StatTile";
import { COLUMN } from "@/pages/ide/Experiments/ExperimentDetail/column";
import type { ExperimentListItem } from "@/types/experiment";
import RetrospectiveOffer from "./RetrospectiveOffer";
import VerbatimRefusal from "./VerbatimRefusal";

/**
 * The design could not be registered. The refusal is shown exactly as
 * the platform wrote it — it names what its own guard tested. The wrapper's
 * testid differs from the alert's (`experiment-refused`) so a lookup by either
 * is unambiguous.
 */
export default function RefusedView({ summary }: { summary: ExperimentListItem }) {
  return (
    <div className={cn(COLUMN, "flex flex-col gap-3")} data-testid='experiment-refused-view'>
      <div className='flex flex-wrap items-center justify-between gap-2'>
        <StatGrid className='w-full'>
          <StatTile
            id='refused-status'
            testId='experiment-refused-status'
            label='Status'
            value='Registration refused'
            tone='danger'
          />
        </StatGrid>
      </div>
      <VerbatimRefusal
        title='Registration refused'
        text={
          summary.registration_refusal ??
          "The platform refused this registration and gave no reason."
        }
        testId='experiment-refused'
      />
      <p className='text-muted-foreground text-xs'>
        Edit the design; the platform re-checks it on its next pass.
      </p>
      {summary.retrospective_ready && <RetrospectiveOffer summary={summary} />}
    </div>
  );
}
