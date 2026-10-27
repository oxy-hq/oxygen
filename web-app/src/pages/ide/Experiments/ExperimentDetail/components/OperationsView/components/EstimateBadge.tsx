import { type Tone, ToneBadge } from "@/pages/ide/Experiments/components/glance/tone";
import { formatInstant } from "@/pages/ide/Experiments/dates";
import { type EstimateState, MAX_ESTIMATE_ATTEMPTS as MAX_ATTEMPTS } from "@/types/experiment";

const VIEW = {
  running: { tone: "info", label: "Estimate running" },
  done: { tone: "success", label: "Estimate done" },
  failed: { tone: "danger", label: "Estimate failed" }
} satisfies Record<string, { tone: Tone; label: string }>;

/**
 * The estimate job's standing as a pill, never an effect number. A failure is
 * retried by the server on its own, so there is no re-run control. The error is
 * the server's own text, shown verbatim.
 */
export default function EstimateBadge({ estimate }: { estimate: EstimateState }) {
  if (estimate.state === "none" || !(estimate.state in VIEW)) return null;
  const view = VIEW[estimate.state];
  return (
    <div className='flex flex-col gap-1' data-testid='experiment-estimate'>
      <div>
        <ToneBadge tone={view.tone} testId='experiment-estimate-state'>
          {view.label}
        </ToneBadge>
      </div>
      {estimate.state === "failed" && (
        <div className='flex flex-col gap-1 text-muted-foreground text-xs'>
          <p>
            Failed ({estimate.task_status}) at {formatInstant(estimate.failed_at)}.{" "}
            {estimate.attempts >= MAX_ATTEMPTS
              ? `The estimate gave up after ${MAX_ATTEMPTS} attempts.`
              : `Attempt ${estimate.attempts} of ${MAX_ATTEMPTS}. It retries automatically.`}
          </p>
          {estimate.error && (
            <pre
              className='whitespace-pre-wrap break-words text-foreground'
              data-testid='experiment-estimate-error'
            >
              {estimate.error}
            </pre>
          )}
        </div>
      )}
    </div>
  );
}
