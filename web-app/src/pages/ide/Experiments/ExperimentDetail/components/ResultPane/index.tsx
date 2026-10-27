import { useExperimentResult, useExperimentRun } from "@/hooks/api/useExperiments";
import { ErrorState, LoadingState } from "@/pages/ide/coordinator/components/PageState";
import type { ExperimentListItem } from "@/types/experiment";
import PendingView from "./components/PendingView";
import ReportBody from "./components/ReportBody";
import SealedView from "./components/SealedView";

/** Mounted only while the report is pending: reads the run for its estimate state. */
function PendingResult({ name, state }: { name: string; state: "reported" | "aborted" }) {
  const { data } = useExperimentRun(name);
  return (
    <PendingView
      state={state}
      estimate={data?.run?.estimate}
      leversKept={data?.run ? data.run.abort_levers === "keep" : undefined}
    />
  );
}

/**
 * The only component that asks for a result. The detail page mounts it for
 * `reported` / `aborted` and nothing else (`viewForState`), which is what keeps
 * the UI from ever requesting an effect early — the server enforces the same
 * rule, and this makes the client incapable of testing it. Both 409 answers are
 * values, and both are re-read until the report lands.
 */
export default function ResultPane({ summary }: { summary: ExperimentListItem }) {
  const { data, isPending, error, refetch } = useExperimentResult(summary.name);

  if (isPending) return <LoadingState />;
  if (error) return <ErrorState message={error.message} onRetry={() => refetch()} />;
  if (!data) return null;
  if (data.kind === "sealed") return <SealedView state={data.state || summary.status} />;
  if (data.kind === "pending") return <PendingResult name={summary.name} state={data.state} />;
  return <ReportBody result={data.result} />;
}
