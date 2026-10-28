import { formatRatio, reportFormat } from "@/pages/ide/Experiments/format";
import { predictionVerdict, priorSourceLabel } from "@/pages/ide/Experiments/prediction";
import type { ReportView } from "@/types/experiment";
import Section from "../../Section";
import HowCalculated from "./HowCalculated";

/**
 * The feedback half of the control loop: what the World Model said the lever
 * would do, against what the experiment measured. The sentence is the server's
 * `statement`, verbatim; the verdict attribute keeps `prior_in_interval`
 * three-valued for styling and tests. When a prior was frozen but could not be
 * compared there is no prediction, only the server's `prediction_note`.
 */
export default function PredictionBlock({ report }: { report: ReportView }) {
  const p = report.prediction;
  if (!p) {
    if (!report.prediction_note) return null;
    return (
      <Section title='Predicted against measured' testId='experiment-report-prediction'>
        <p className='whitespace-pre-wrap text-sm' data-testid='experiment-report-prediction-note'>
          {report.prediction_note}
        </p>
      </Section>
    );
  }
  const fmt = reportFormat(report);
  return (
    <Section title='Predicted against measured' testId='experiment-report-prediction'>
      <dl className='grid grid-cols-[max-content_1fr] gap-x-6 gap-y-1 text-sm'>
        <dt className='text-muted-foreground'>World Model slope</dt>
        <dd data-testid='experiment-report-prior'>
          {formatRatio(p.prior.value)} per driver unit, {priorSourceLabel(p.prior.source)}
        </dd>
        <dt className='text-muted-foreground'>Predicted</dt>
        <dd data-testid='experiment-report-predicted'>{fmt.signed(p.predicted)}</dd>
        <dt className='text-muted-foreground'>Measured</dt>
        <dd data-testid='experiment-report-measured'>
          {fmt.signed(p.measured)}, interval{" "}
          {fmt.interval(p.measured_interval.low, p.measured_interval.high)}
        </dd>
      </dl>
      <p
        className='mt-2 mb-2 whitespace-pre-wrap font-medium text-sm'
        data-testid='experiment-report-prediction-statement'
        data-verdict={predictionVerdict(p)}
      >
        {p.statement}
      </p>
      <HowCalculated>
        <p>
          Predicted is the slope times the driver move the experiment measured, not one that was
          planned, so it tests the slope itself.
        </p>
      </HowCalculated>
    </Section>
  );
}
