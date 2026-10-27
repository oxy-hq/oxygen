import { formatInstant } from "@/pages/ide/Experiments/dates";
import { type EstimateState, MAX_ESTIMATE_ATTEMPTS as MAX_ATTEMPTS } from "@/types/experiment";
import Section from "../../Section";

/**
 * The estimate job's standing — never an effect number. A failure is retried by
 * the server on its own, so there is deliberately no re-run control. The error
 * is the server's own text, shown verbatim.
 */
export default function EstimateStatus({ estimate }: { estimate: EstimateState }) {
  if (estimate.state === "none") return null;
  return (
    <Section title='Estimate' testId='experiment-estimate'>
      {estimate.state === "running" && <p className='text-sm'>Estimate running</p>}
      {estimate.state === "done" && <p className='text-sm'>Estimate done</p>}
      {estimate.state === "failed" && (
        <div className='flex flex-col gap-1 text-sm'>
          <p className='font-medium'>
            Estimate failed ({estimate.task_status}) at {formatInstant(estimate.failed_at)}.
          </p>
          <p>
            {estimate.attempts >= MAX_ATTEMPTS
              ? `The estimate gave up after ${MAX_ATTEMPTS} attempts.`
              : `Attempt ${estimate.attempts} of ${MAX_ATTEMPTS}. It retries automatically.`}
          </p>
          {estimate.error && (
            <pre
              className='whitespace-pre-wrap break-words text-xs'
              data-testid='experiment-estimate-error'
            >
              {estimate.error}
            </pre>
          )}
        </div>
      )}
    </Section>
  );
}
