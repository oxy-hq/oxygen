import { Loader2, RefreshCw } from "lucide-react";
import { Alert, AlertDescription } from "@/components/ui/shadcn/alert";
import { Button } from "@/components/ui/shadcn/button";
import { useExperimentInterim, useRequestInterim } from "@/hooks/api/useExperiments";
import { Hinted, HintIcon } from "@/pages/ide/Experiments/components/glance/Hinted";
import { ToneBadge } from "@/pages/ide/Experiments/components/glance/tone";
import { formatInstant } from "@/pages/ide/Experiments/dates";
import type { InterimReading, ResultResponse } from "@/types/experiment";
import ReportSummary from "../../ResultPane/components/ReportSummary";
import SeriesBlock from "../../ResultPane/components/SeriesBlock";

const CAVEAT =
  "This is the registered estimator run on the days so far. The test was planned to be read once, at the end. Checking it while it runs makes a chance swing look real more often than the stated confidence says, so treat a 'significant' interim as a hint, not a result. Only the final report counts. An interim never changes the levers, stops the run, or writes anything back.";

/**
 * Adapts an interim reading to the shape `ReportSummary` reads. Only `report`,
 * `design_id` and `created_at` are read there; the write-back and prediction
 * slots are filled so nothing downstream can offer a write-back.
 */
function asResult(experiment: string, runId: string, designId: string, r: InterimReading) {
  const result: ResultResponse = {
    experiment,
    run_id: runId,
    state: "reported",
    design_id: designId,
    aborted: false,
    created_at: r.computed_at,
    report: r.report,
    effect: {},
    ratio: null,
    compliance: r.compliance,
    write_back: {
      available: false,
      reason: "an interim reading never proposes a coefficient",
      edit: null
    },
    directional: r.report.directional,
    alpha: r.report.alpha,
    prediction: null,
    assumptions: r.report.assumptions
  };
  return result;
}

/**
 * A labelled peek at a live / closing run: the registered estimator over the days
 * finished so far. Reuses the report's summary and day-by-day chart, and renders
 * nothing that writes back. The raw `effect` is never read.
 */
export default function InterimPanel({ name }: { name: string }) {
  const { data } = useExperimentInterim(name, true);
  const request = useRequestInterim(name);
  if (data?.kind !== "interim") return null;
  const { interim } = data;
  const reading = interim.reading;
  const busy = interim.running || request.isPending;

  return (
    <section className='flex flex-col gap-3 rounded-lg border p-4' data-testid='experiment-interim'>
      <div className='flex flex-wrap items-center justify-between gap-2'>
        <div className='flex flex-wrap items-center gap-2'>
          <ToneBadge tone='warning' testId='experiment-interim-badge'>
            Interim, not final
          </ToneBadge>
          <HintIcon hint={CAVEAT} testId='experiment-interim-caveat-hint' />
          {reading && (
            <span className='text-muted-foreground text-xs' data-testid='experiment-interim-meta'>
              Read through {reading.through} · computed {formatInstant(reading.computed_at)}
            </span>
          )}
        </div>
        <Hinted hint='Read the days finished so far again, now.'>
          <span>
            <Button
              variant='outline'
              size='sm'
              disabled={busy}
              onClick={() => request.mutate()}
              data-testid='experiment-interim-refresh'
            >
              {busy ? <Loader2 className='animate-spin' /> : <RefreshCw />}
              {busy ? "Reading…" : "Refresh"}
            </Button>
          </span>
        </Hinted>
      </div>

      {request.error && (
        <Alert data-testid='experiment-interim-request-error'>
          <AlertDescription>{request.error.message}</AlertDescription>
        </Alert>
      )}
      {interim.error && (
        <Alert data-testid='experiment-interim-error'>
          <AlertDescription>Last attempt: {interim.error}</AlertDescription>
        </Alert>
      )}

      {reading ? (
        <>
          <ReportSummary
            result={asResult(interim.experiment, interim.run_id, interim.design_id, reading)}
          />
          {reading.series && <SeriesBlock series={reading.series} />}
        </>
      ) : (
        !interim.running && (
          <p className='text-muted-foreground text-sm' data-testid='experiment-interim-empty'>
            No interim yet. The first one is read once a whole post-window day has finished; it
            refreshes daily.
          </p>
        )
      )}
    </section>
  );
}
