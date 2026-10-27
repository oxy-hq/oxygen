import { Loader2 } from "lucide-react";
import { cn } from "@/libs/shadcn/utils";
import { ToneBadge } from "@/pages/ide/Experiments/components/glance/tone";
import { COLUMN } from "@/pages/ide/Experiments/ExperimentDetail/column";
import { statusLabel } from "@/pages/ide/Experiments/viewForState";
import { type EstimateState, MAX_ESTIMATE_ATTEMPTS } from "@/types/experiment";
import EstimateStatus from "../../OperationsView/components/EstimateStatus";
import AbortLeversNote from "./AbortLeversNote";

/** The badge's wording and tone: preparing, or the estimate's own failure with its attempt count. */
function badge(estimate?: EstimateState) {
  if (estimate?.state === "failed") {
    const gaveUp = estimate.attempts >= MAX_ESTIMATE_ATTEMPTS;
    return {
      tone: "danger" as const,
      label: gaveUp
        ? `Estimate failed · gave up after ${MAX_ESTIMATE_ATTEMPTS} attempts`
        : `Estimate failed · attempt ${estimate.attempts} of ${MAX_ESTIMATE_ATTEMPTS}`
    };
  }
  return { tone: "info" as const, label: "Preparing report" };
}

/**
 * `GET /result` answered 409 `{ sealed: false, pending: true }`: the run is
 * terminal but its estimate is not written yet. Not sealed and not an error —
 * the report is being prepared, and the page polls until it lands. The estimate
 * job's standing is shown beneath, so a failed estimate is never a silent spinner.
 */
export default function PendingView({
  state,
  estimate,
  leversKept
}: {
  state: string;
  estimate?: EstimateState;
  /** An aborted run: whether its levers were left as they were. Unknown until the run is read. */
  leversKept?: boolean;
}) {
  const { tone, label } = badge(estimate);
  return (
    <div className={cn(COLUMN, "flex flex-col gap-3")} data-testid='experiment-report-pending-view'>
      <div
        role='status'
        className='flex flex-wrap items-center gap-2 text-muted-foreground text-sm'
        data-testid='experiment-report-pending'
      >
        <ToneBadge tone={tone} testId='experiment-report-pending-badge'>
          {label}
        </ToneBadge>
        <Loader2 className='h-4 w-4 animate-spin' />
        <span>
          The run is {statusLabel(state).toLowerCase()} and its report is being prepared. It will
          appear here as soon as it is written.
        </span>
      </div>
      {state === "aborted" && leversKept !== undefined && (
        <div className='text-muted-foreground text-sm'>
          <AbortLeversNote kept={leversKept} />
        </div>
      )}
      {estimate && <EstimateStatus estimate={estimate} />}
    </div>
  );
}
