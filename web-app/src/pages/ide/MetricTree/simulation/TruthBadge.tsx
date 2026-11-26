import { cn } from "@/libs/shadcn/utils";
import type { Outcome, SimulationFit } from "@/types/simulation";
import { EdgeLabel } from "./EdgeLabel";

/**
 * One edge's fit at one period: what the model estimated, what the world
 * actually is, and how far apart those are.
 *
 * This is the one thing this canvas can show that no real workspace ever can —
 * on customer data `converged` and `confidently wrong` are the same response
 * byte for byte.
 */

const OUTCOME_LABEL: Record<Outcome, string> = {
  refused: "Refused",
  converged: "Converged",
  confidently_wrong: "Confidently wrong"
};

/** Not a palette choice — a claim. `confidently_wrong` is the only outcome that
 *  costs a customer money, so it is the only one that reads as an alarm.
 *  Emerald is reserved for workflow-node success and is deliberately not used. */
const OUTCOME_CLASS: Record<Outcome, string> = {
  refused: "bg-muted text-muted-foreground",
  converged: "bg-primary/10 text-primary",
  confidently_wrong: "bg-destructive/10 text-destructive"
};

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <div className='flex min-w-0 flex-col'>
      <dt className='text-[9.5px] text-muted-foreground'>{label}</dt>
      <dd className='truncate font-mono text-foreground text-xs tabular-nums'>{value}</dd>
    </div>
  );
}

export function TruthBadge({ fit }: { fit: SimulationFit }) {
  const ratio =
    fit.coefficient !== null && fit.true_local_slope !== 0
      ? fit.coefficient / fit.true_local_slope
      : null;

  return (
    <div className='flex flex-col gap-1.5 border border-border bg-background/40 p-2'>
      <div className='flex items-start justify-between gap-2'>
        <EdgeLabel edge={fit.edge} className='flex-1' />
        {/* `whitespace-nowrap`, not wrapping: "Confidently wrong" broken across
            two lines reads as two separate words rather than one verdict. The
            edge beside it is what gives up width. */}
        <span
          className={cn(
            "shrink-0 whitespace-nowrap px-1.5 py-0.5 text-[9.5px] uppercase tracking-wider",
            OUTCOME_CLASS[fit.outcome]
          )}
        >
          {OUTCOME_LABEL[fit.outcome]}
        </span>
      </div>

      {fit.coefficient === null ? (
        // A refusal is not a zero, and it is not an error either — the model
        // declined, which costs an opportunity rather than money. Show why.
        <p className='text-[11px] text-muted-foreground leading-relaxed'>
          {fit.refusal ?? "no coefficient"}
        </p>
      ) : (
        <dl className='grid grid-cols-3 gap-2'>
          <Stat label='β̂' value={fit.coefficient.toFixed(3)} />
          <Stat label='β true' value={fit.true_local_slope.toFixed(3)} />
          <Stat label='ratio' value={ratio === null ? "—" : `${ratio.toFixed(2)}×`} />
        </dl>
      )}

      <p className='text-[9.5px] text-muted-foreground'>
        {fit.form} basis · {fit.n.toLocaleString()} pairs · {fit.n_panels} panels
      </p>
    </div>
  );
}
