import { cn } from "@/libs/shadcn/utils";
import { COLUMN } from "@/pages/ide/Experiments/ExperimentDetail/column";
import type { ResultResponse } from "@/types/experiment";
import AssumptionsBlock from "./AssumptionsBlock";
import PredictionBlock from "./PredictionBlock";
import ReportDetails from "./ReportDetails";
import ReportSummary from "./ReportSummary";
import SeriesBlock from "./SeriesBlock";
import WriteBackSection from "./WriteBackSection";

/**
 * The report exactly as the API returns it. Only `result.report` (and the
 * write-back availability) is read: `result.effect` and `result.ratio` are audit
 * fields and are never rendered. The summary card leads, so the result reads at a
 * glance; the secondary detail is folded into one collapsed section below it. An estimator refusal
 * replaces the effect numbers rather than sitting beside a `—`; the day-by-day
 * series still shows, since it is what was read, not what was estimated.
 */
export default function ReportBody({ result }: { result: ResultResponse }) {
  const { report } = result;
  return (
    <div
      className={cn(COLUMN, "flex flex-col gap-4")}
      data-testid='experiment-report'
      data-report-state={result.state}
    >
      <ReportSummary result={result} />
      {result.series && (
        <SeriesBlock
          series={result.series}
          retrospective={result.report.status.kind === "retrospective"}
        />
      )}
      <AssumptionsBlock assumptions={report.assumptions} />
      <PredictionBlock report={report} />
      <WriteBackSection result={result} />
      <ReportDetails result={result} />
    </div>
  );
}
