import { useState } from "react";
import { cn } from "@/libs/shadcn/utils";
import { StatGrid, StatTile } from "@/pages/ide/Experiments/components/glance/StatTile";
import { ToneBadge } from "@/pages/ide/Experiments/components/glance/tone";
import {
  formatAlpha,
  formatShare,
  formatUnitDay,
  NOT_AVAILABLE
} from "@/pages/ide/Experiments/format";
import type { Grain } from "@/pages/ide/Experiments/grain";
import type { Proposal } from "@/types/experiment";
import VerbatimRefusal from "../../VerbatimRefusal";
import {
  describeCandidate,
  formatOwnMde,
  GROUP_CAVEAT,
  isGroupDesign,
  rankCandidates
} from "../compare";
import CandidateDetail, { type AdoptControl } from "./CandidateDetail";
import Warnings from "./Warnings";

const NO_REASON = "The platform gave no reason.";

/** A refusal or fault, titled by the candidate it belongs to, text verbatim. */
function Failures({
  proposals,
  kind,
  grain
}: {
  proposals: Proposal[];
  kind: "refusal" | "failure";
  grain: Grain;
}) {
  if (proposals.length === 0) return null;
  const title = kind === "refusal" ? "Pricing refused" : "Pricing failed";
  return (
    <div className='flex flex-col gap-3' data-testid={`experiment-price-${kind}s`}>
      {proposals.map((p, i) => (
        <div key={p.id} className='flex flex-col gap-2'>
          <VerbatimRefusal
            title={`${title}: ${describeCandidate(p.request, grain)} (α = ${formatAlpha(p.alpha)})`}
            text={p.refusal ?? NO_REASON}
            testId={`experiment-price-${kind}-${i + 1}`}
          />
          <Warnings warnings={p.warnings} testId={`experiment-price-${kind}-warnings-${i + 1}`} />
        </div>
      ))}
    </div>
  );
}

/**
 * A settled request, candidates side by side. The priced ones are a table ranked
 * on MDE per unit-day (the one comparable scale), the top one selected; each is
 * also shown in the design's own unit, which is never sorted on. Refusals and
 * faults are not behind the table: they are content, per candidate, verbatim —
 * a refusal is the platform's most valuable output.
 */
export default function PriceOutcome({
  proposals,
  adopt,
  grain = "day"
}: {
  proposals: Proposal[];
  adopt?: AdoptControl;
  /** The draft's `design.grain`: every MDE is per unit-period of it. */
  grain?: Grain;
}) {
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const priced = rankCandidates(proposals).filter((r) => r.proposal.status === "done");
  const refused = proposals.filter((p) => p.status === "refused");
  const failed = proposals.filter((p) => p.status === "failed");
  const selected =
    priced.find((r) => r.proposal.id === selectedId)?.proposal ?? priced[0]?.proposal ?? null;
  const outcome = priced.length > 0 ? "priced" : refused.length > 0 ? "refused" : "failed";

  return (
    <section
      className='flex flex-col gap-4'
      data-testid='experiment-price-outcome'
      data-outcome={outcome}
      data-priced={priced.length}
      data-refused={refused.length}
      data-failed={failed.length}
    >
      <div className='flex flex-wrap items-center gap-2' data-testid='experiment-price-summary'>
        {priced.length > 0 && <ToneBadge tone='success'>{priced.length} priced</ToneBadge>}
        {refused.length > 0 && <ToneBadge tone='danger'>{refused.length} refused</ToneBadge>}
        {failed.length > 0 && <ToneBadge tone='danger'>{failed.length} failed</ToneBadge>}
      </div>
      {priced.length > 0 && (
        <div className='flex flex-col gap-2'>
          <div className='grid gap-3 sm:grid-cols-2' data-testid='experiment-price-comparison'>
            {priced.map(({ proposal, rank }, i) => {
              const position = i + 1;
              const isSelected = proposal.id === selected?.id;
              const group = isGroupDesign(proposal);
              return (
                <div
                  key={proposal.id}
                  className={cn(
                    "flex flex-col gap-3 rounded-lg border bg-card p-3",
                    isSelected && "border-primary ring-1 ring-primary"
                  )}
                  data-testid={`experiment-candidate-${position}`}
                  data-proposal-id={proposal.id}
                  data-selected={isSelected}
                >
                  <div className='flex flex-wrap items-center gap-2'>
                    <span className='font-semibold text-muted-foreground text-xs tabular-nums'>
                      #{rank ?? NOT_AVAILABLE}
                    </span>
                    {rank === 1 && (
                      <ToneBadge tone='success' testId='experiment-candidate-best'>
                        Best ranked
                      </ToneBadge>
                    )}
                    {proposal.warnings.length > 0 && (
                      <span data-testid={`experiment-candidate-warning-count-${position}`}>
                        <ToneBadge tone='warning'>
                          {proposal.warnings.length}{" "}
                          {proposal.warnings.length === 1 ? "warning" : "warnings"}
                        </ToneBadge>
                      </span>
                    )}
                  </div>
                  <button
                    type='button'
                    className='text-left font-medium text-sm hover:underline'
                    aria-pressed={isSelected}
                    onClick={() => setSelectedId(proposal.id)}
                    data-testid={`experiment-candidate-select-${position}`}
                  >
                    {describeCandidate(proposal.request, grain)}
                  </button>
                  <div>
                    <div
                      className='font-semibold text-2xl tabular-nums'
                      data-testid={`experiment-candidate-mde-unit-day-${position}`}
                    >
                      {formatUnitDay(proposal.mde_unit_day, grain)}
                    </div>
                    <div className='text-muted-foreground text-xs'>
                      MDE per unit-{grain} (ranked on this)
                    </div>
                  </div>
                  <StatGrid className='sm:grid-cols-3'>
                    <StatTile
                      id={`own-${position}`}
                      label={group ? "Own unit" : "MDE"}
                      value={
                        <span data-testid={`experiment-candidate-mde-${position}`}>
                          {formatOwnMde(proposal, grain)}
                        </span>
                      }
                      sub={
                        <span data-testid={`experiment-candidate-unit-${position}`}>
                          {group ? `per group-${grain}, not comparable` : `per unit-${grain}`}
                        </span>
                      }
                      tone={group ? "warning" : undefined}
                    />
                    <StatTile
                      id={`alpha-${position}`}
                      label='α'
                      value={formatAlpha(proposal.alpha)}
                    />
                    <StatTile
                      id={`baseline-${position}`}
                      label='Of baseline'
                      value={formatShare(proposal.mde_relative)}
                    />
                  </StatGrid>
                </div>
              );
            })}
          </div>
          <p className='text-muted-foreground text-xs'>
            Ranked by MDE per unit-{grain}, the smallest effect each design could read on one scale.
            Choose a card for its assignment and a preview of its keys.
          </p>
          {priced.some((r) => isGroupDesign(r.proposal)) && (
            <p
              className='text-muted-foreground text-xs'
              data-testid='experiment-candidate-group-caveat'
            >
              {GROUP_CAVEAT}
            </p>
          )}
        </div>
      )}
      {selected && <CandidateDetail proposal={selected} adopt={adopt} grain={grain} />}
      <Failures proposals={refused} kind='refusal' grain={grain} />
      <Failures proposals={failed} kind='failure' grain={grain} />
    </section>
  );
}
