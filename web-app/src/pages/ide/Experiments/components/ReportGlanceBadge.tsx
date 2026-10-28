import { ToneBadge } from "@/pages/ide/Experiments/components/glance/tone";
import {
  plainVerdict,
  verdictTone
} from "@/pages/ide/Experiments/ExperimentDetail/components/ResultPane/components/ReportSummary/badges";
import { reportFormat } from "@/pages/ide/Experiments/format";
import type { ReportGlance } from "@/types/experiment";

/**
 * A reported row's result in one line: the report page's plain verdict, in its
 * colour, and the gross effect. The engine's exact verdict goes on hover.
 */
export default function ReportGlanceBadge({
  name,
  glance
}: {
  name: string;
  glance: ReportGlance;
}) {
  const h = glance.headline;
  const refused = glance.refusal !== null;
  return (
    <span
      className='inline-flex flex-wrap items-center gap-1.5'
      data-testid={`experiment-row-glance-${name}`}
    >
      {refused ? (
        <ToneBadge tone='danger' testId={`experiment-row-verdict-${name}`}>
          Estimator refused
        </ToneBadge>
      ) : (
        h && (
          <>
            <span title={h.verdict} className='inline-flex'>
              <ToneBadge tone={verdictTone(glance)} testId={`experiment-row-verdict-${name}`}>
                {plainVerdict(glance)}
              </ToneBadge>
            </span>
            <span
              className='font-medium text-sm tabular-nums'
              data-testid={`experiment-row-effect-${name}`}
            >
              {reportFormat(glance).signed(h.estimate)}
            </span>
          </>
        )
      )}
    </span>
  );
}
