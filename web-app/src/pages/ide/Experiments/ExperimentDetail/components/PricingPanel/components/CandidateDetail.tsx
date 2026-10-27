import { Button } from "@/components/ui/shadcn/button";
import { StatGrid, StatTile } from "@/pages/ide/Experiments/components/glance/StatTile";
import {
  formatAlpha,
  formatRatio,
  formatShare,
  formatUnitDay
} from "@/pages/ide/Experiments/format";
import type { Grain } from "@/pages/ide/Experiments/grain";
import { priorSourceLabel } from "@/pages/ide/Experiments/prediction";
import type { Proposal } from "@/types/experiment";
import {
  describeCandidate,
  formatOwn,
  formatOwnMde,
  GROUP_CAVEAT,
  isGroupDesign
} from "../compare";
import Warnings from "./Warnings";

/** The assignment: a switchback's schedule in pairs, or the waves (of groups, under `assign_by`). */
function Assignment({ proposal }: { proposal: Proposal }) {
  if (proposal.periods) {
    return (
      <ul className='text-sm' data-testid='experiment-candidate-periods'>
        {proposal.periods.map((p, i) => (
          <li key={p.from}>
            Pair {Math.floor(i / 2) + 1} · {p.from} · {p.state === "on" ? "On" : "Off"}
          </li>
        ))}
      </ul>
    );
  }
  if (!proposal.waves) return null;
  const [one, many] = isGroupDesign(proposal) ? ["group", "groups"] : ["unit", "units"];
  return (
    <ul className='text-sm' data-testid='experiment-candidate-waves'>
      {proposal.waves.map((w) => (
        <li key={w.switch_on}>
          Switch on {w.switch_on}: {w.units.length} {w.units.length === 1 ? one : many}
        </li>
      ))}
    </ul>
  );
}

/** What "Use this design" needs from the page: the permission and the adopt call. */
export interface AdoptControl {
  /** Whether this person may write designs. */
  canWrite: boolean;
  pending: boolean;
  onUse: (proposalId: string) => Promise<void>;
}

/** The note shown in place of "Use this design" once the draft changed after pricing. */
export const STALE_NOTE = "The draft changed after this was priced. Price it again to use it.";

/** Why a done proposal cannot be adopted, or null when it can. */
export function adoptBlockedReason(adopt: AdoptControl): string | null {
  if (!adopt.canWrite) return "Viewers cannot change a design.";
  return null;
}

/**
 * One priced candidate in full: its MDE on both scales, its assignment, what the
 * platform warned about, what using it would save and, when adoptable,
 * the "Use this design" action. The caption under the per-unit-day figure is the
 * group caveat whenever the design is a group design.
 */
export default function CandidateDetail({
  proposal,
  adopt,
  grain = "day"
}: {
  proposal: Proposal;
  adopt?: AdoptControl;
  grain?: Grain;
}) {
  const blocked = adopt ? adoptBlockedReason(adopt) : null;
  const group = isGroupDesign(proposal);
  return (
    <section
      className='flex flex-col gap-4 rounded-lg border p-4'
      data-testid='experiment-candidate-detail'
      data-proposal-id={proposal.id}
    >
      <div>
        <h3 className='font-medium text-sm'>{describeCandidate(proposal.request, grain)}</h3>
        <p className='text-muted-foreground text-xs'>Priced at α = {formatAlpha(proposal.alpha)}</p>
      </div>
      <StatGrid>
        <StatTile
          id='detail-mde'
          label='Minimum detectable effect'
          value={
            <span data-testid='experiment-candidate-mde'>{formatOwnMde(proposal, grain)}</span>
          }
          sub={group ? "design's own unit" : undefined}
          tone={group ? "warning" : undefined}
        />
        <StatTile
          id='detail-mde-unit-day'
          label={`Per unit-${grain}`}
          value={
            <span data-testid='experiment-candidate-mde-unit-day'>
              {formatUnitDay(proposal.mde_unit_day, grain)}
            </span>
          }
          hint={group ? GROUP_CAVEAT : undefined}
        />
        <StatTile
          id='detail-mde-relative'
          label='Relative to baseline'
          value={
            <span data-testid='experiment-candidate-mde-relative'>
              {formatShare(proposal.mde_relative)}
            </span>
          }
          sub={`of the baseline ${formatOwn(proposal, proposal.baseline, grain)}`}
        />
      </StatGrid>
      {group && <p className='text-muted-foreground text-xs'>{GROUP_CAVEAT}</p>}
      {proposal.prior_coefficient && (
        <p className='text-sm' data-testid='experiment-candidate-prior'>
          <span className='text-muted-foreground'>World Model slope: </span>
          {formatRatio(proposal.prior_coefficient.value)} per driver unit,{" "}
          {priorSourceLabel(proposal.prior_coefficient.source)}. Using this design freezes it in, so
          the report can set it against the measurement.
        </p>
      )}
      <Assignment proposal={proposal} />
      {proposal.assign_by && (
        <p className='text-muted-foreground text-sm'>
          Randomised by <code className='font-mono'>{proposal.assign_by}</code>: the assignment
          lists groups, not single units.
        </p>
      )}
      {proposal.strata && (
        <p className='text-muted-foreground text-sm' data-testid='experiment-candidate-strata'>
          {proposal.request.shape === "waves" && proposal.request.pairs
            ? `Matched in ${proposal.strata.length} pairs by baseline volume; one of each pair is treated.`
            : `Blocked by baseline volume into ${proposal.strata.length} blocks; each wave takes its share of every block.`}
        </p>
      )}
      <Warnings warnings={proposal.warnings} />
      <p className='text-muted-foreground text-sm' data-testid='experiment-candidate-saves'>
        Using this design saves the assignment above, its seed and the promised minimum detectable
        effect into the draft design. Registering it must happen before the first switch date (or
        first period), less the anticipation days.
      </p>
      {adopt && proposal.status === "done" && proposal.stale && (
        <p className='text-muted-foreground text-xs' data-testid='experiment-use-design-stale'>
          {STALE_NOTE}
        </p>
      )}
      {adopt && proposal.status === "done" && !proposal.stale && (
        <div className='flex flex-col gap-1'>
          <div>
            <Button
              size='sm'
              disabled={blocked !== null || adopt.pending}
              onClick={() => void adopt.onUse(proposal.id)}
              data-testid='experiment-use-design'
            >
              Use this design
            </Button>
          </div>
          {blocked && (
            <p className='text-muted-foreground text-xs' data-testid='experiment-use-design-reason'>
              {blocked}
            </p>
          )}
        </div>
      )}
    </section>
  );
}
