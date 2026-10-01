import { Spinner } from "@/components/ui/shadcn/spinner";
import { usePreviewRun } from "@/hooks/api/workspaces/usePreviews";
import PreviewCompareView from "./PreviewCompareView";
import PreviewRunStepRow from "./PreviewRunStepRow";
import PreviewSampleView from "./PreviewSampleView";

/**
 * A selected run's detail, polling while the run hasn't finished. A `compare`
 * or `airway_sample` run has no steps of its own (`steps: []`) — its whole
 * detail is the `compare`/`sample` result, so the steps section (and its
 * "No steps yet" fallback) is skipped for those kinds rather than shown
 * empty above the real content. A `transform_build` shows both: its own
 * steps, then its linked compare once one is queued.
 */
export default function PreviewRunDetailView({
  workspaceId,
  runId
}: {
  workspaceId: string;
  runId: string;
}) {
  const { data, isLoading, error } = usePreviewRun(workspaceId, runId);

  if (isLoading) return <Spinner className='size-4 text-muted-foreground' />;
  if (error) return <p className='text-destructive text-xs'>{error.message}</p>;
  if (!data) return null;

  const showsSteps = data.kind !== "compare" && data.kind !== "airway_sample";

  return (
    <div className='flex flex-col gap-2' data-testid={`preview-run-detail-${runId}`}>
      {data.error && <p className='text-destructive text-xs'>{data.error}</p>}
      {showsSteps &&
        (data.steps.length === 0 ? (
          <p className='text-muted-foreground text-xs'>No steps yet.</p>
        ) : (
          data.steps.map((step) => <PreviewRunStepRow key={step.name} step={step} />)
        ))}
      {data.compare && <PreviewCompareView compare={data.compare} />}
      {data.sample && <PreviewSampleView sample={data.sample} />}
    </div>
  );
}
