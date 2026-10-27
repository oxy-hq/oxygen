import { formatRatio, formatRatioInterval } from "@/pages/ide/Experiments/format";
import { priorSourceLabel } from "@/pages/ide/Experiments/prediction";
import type { ResultResponse } from "@/types/experiment";
import Section from "../../Section";
import HowCalculated from "./HowCalculated";
import WriteBackAction from "./WriteBackAction";

/**
 * The lever as an instrument: a proposed slope onto a driver edge, or the reason
 * nothing is proposed. Renders nothing for an experiment with no driver.
 */
export default function WriteBackSection({ result }: { result: ResultResponse }) {
  const { write_back: wb, report } = result;

  if (wb.available && wb.edit) {
    const { edit } = wb;
    const ratio = report.ratio;
    return (
      <Section title='Proposed slope for the semantic model' testId='experiment-report-patch'>
        <dl className='grid grid-cols-[max-content_1fr] gap-x-6 gap-y-1 text-sm'>
          <dt className='text-muted-foreground'>Driver edge</dt>
          <dd>
            {edit.driver_measure} → {edit.target_measure}
          </dd>
          <dt className='text-muted-foreground'>Coefficient</dt>
          <dd>{formatRatio(edit.coefficient)}</dd>
          {ratio && (
            <>
              <dt className='text-muted-foreground'>Interval</dt>
              <dd>{formatRatioInterval(ratio.interval.low, ratio.interval.high)}</dd>
            </>
          )}
          <dt className='text-muted-foreground'>File</dt>
          <dd className='font-mono'>{edit.file_path}</dd>
          <dt className='text-muted-foreground'>Provenance</dt>
          <dd className='font-mono'>{edit.refs_entry}</dd>
          {edit.replaces && (
            <>
              <dt className='text-muted-foreground'>Replaces</dt>
              <dd data-testid='experiment-report-patch-replaces'>
                {formatRatio(edit.replaces.value)} ({priorSourceLabel(edit.replaces.source)})
                {edit.replaces.edge_coefficient !== null &&
                  `; the edge's own coefficient is ${formatRatio(edit.replaces.edge_coefficient)}`}
              </dd>
            </>
          )}
        </dl>
        <div className='mt-3'>
          <HowCalculated>
            <p>
              The ratio of the lever's effect on the target to its effect on the driver, as a patch
              against the driver edge. Applying it creates a branch for you to review and merge; the
              platform never writes to main.
            </p>
          </HowCalculated>
        </div>
        <div className='mt-3'>
          <WriteBackAction name={result.experiment} />
        </div>
      </Section>
    );
  }

  return null;
}
