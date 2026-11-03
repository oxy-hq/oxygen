import { Badge } from "@/components/ui/shadcn/badge";
import { StatGrid, StatTile } from "@/pages/ide/Experiments/components/glance/StatTile";
import type { Tone } from "@/pages/ide/Experiments/components/glance/tone";
import {
  complianceLabel,
  formatAlpha,
  formatAmountInterval,
  formatShare,
  formatSignedAmount,
  leakLabel
} from "@/pages/ide/Experiments/format";
import { grainOf, periods } from "@/pages/ide/Experiments/grain";
import type { ReportView } from "@/types/experiment";
import { alphaInWords, confidencePct, verdictTone } from "./badges";
import { HINTS } from "./hints";

/** Three decimals, and below that a bound: `p = 0.000` reads as impossible. */
const formatP = (p: number) => (p < 0.001 ? "p < 0.001" : `p = ${p.toFixed(3)}`);

/** The numbers a reader looks for first, each under a plain label. The projection is always tagged an assumption. */
export default function StatTiles({ report }: { report: ReportView }) {
  const { headline: h, treated_total: treated, projection, compliance: c } = report;
  const compliance = complianceLabel(c);
  const grain = grainOf(report.grain);
  const interim = report.status.kind === "interim";
  // Under a family of outcomes the engine decided at the per-outcome α′, not α.
  const decidedAt = report.alpha_per_outcome ?? report.alpha;
  const pct = confidencePct(decidedAt) ?? `α = ${formatAlpha(decidedAt)}`;
  const bar = report.alpha_per_outcome == null ? pct : `per-outcome ${pct}`;
  const complianceTone: Tone = !compliance.measured
    ? "neutral"
    : c.below_floor
      ? "warning"
      : "success";
  return (
    <StatGrid testId='experiment-report-stats'>
      {h && (
        <StatTile
          id='confidence'
          label='Confidence'
          hint={HINTS.confidence(alphaInWords(report.alpha))}
          tone={verdictTone(report)}
          value={
            interim
              ? `${h.significant ? "Passes" : "Misses"} so far`
              : `${h.significant ? "Passes" : "Misses"} the ${bar} bar`
          }
          sub={[
            h.p_value === null ? "p-value not available" : formatP(h.p_value),
            interim ? `${bar} bar` : null
          ]
            .filter(Boolean)
            .join(" · ")}
        />
      )}
      {report.status.kind === "interim" && (
        <StatTile
          id='read-through'
          testId='experiment-report-read-through'
          label='Read through'
          value={report.status.through}
          sub='the run is still in progress'
          hint='The last whole day the interim read. A post window still running is shortened to the days finished, and a wave with no finished post day yet is left out.'
        />
      )}
      {treated && (
        <StatTile
          id='treated-total'
          testId='experiment-report-treated-total'
          label='Total over treated units'
          hint={HINTS.treatedTotal}
          value={formatSignedAmount(treated.total)}
          sub={formatAmountInterval(treated.interval.low, treated.interval.high)}
        />
      )}
      {treated && (
        <StatTile
          id='treated-units'
          label='Treated units'
          value={treated.units}
          hint={HINTS.treatedUnits}
        />
      )}
      {projection && (
        <StatTile
          id='projection'
          testId='experiment-report-projection'
          label='Projection to untreated units'
          hint={HINTS.projection}
          tag={
            <Badge
              variant='outline'
              className='px-1.5 py-0 font-normal'
              data-testid='experiment-report-assumption'
            >
              Assumption
            </Badge>
          }
          value={formatSignedAmount(projection.total)}
          sub={`${projection.untreated_units} units × ${periods(projection.days, grain)}`}
        />
      )}
      {treated && (
        <StatTile
          id='window'
          label='Measured window'
          value={periods(treated.days, grain)}
          hint={HINTS.window}
        />
      )}
      <StatTile
        id='compliance'
        label='Compliance'
        hint={HINTS.compliance}
        value={compliance.text}
        tone={complianceTone}
        sub={
          compliance.measured
            ? `Floor ${formatShare(c.floor)} · control leakage ${leakLabel(c)}`
            : undefined
        }
      />
    </StatGrid>
  );
}
