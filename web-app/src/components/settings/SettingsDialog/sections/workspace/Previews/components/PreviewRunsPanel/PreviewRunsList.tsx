import { Spinner } from "@/components/ui/shadcn/spinner";
import { usePreviewRuns } from "@/hooks/api/workspaces/usePreviews";
import { isPreviewRunsDisabled } from "@/libs/utils/preview";
import { groupPreviewRuns } from "./groupRuns";
import PreviewRunRow from "./PreviewRunRow";

/**
 * Runs for this branch, newest first — polls itself while any is in flight.
 * A `compare` nests under its `transform_build` (see `groupPreviewRuns`);
 * everything else, `procedure` runs included, stays top-level.
 */
export default function PreviewRunsList({
  workspaceId,
  branch
}: {
  workspaceId: string;
  branch: string;
}) {
  const { data, isLoading, error } = usePreviewRuns(workspaceId, branch);

  if (isLoading) return <Spinner className='size-4 text-muted-foreground' />;
  if (error) {
    return (
      <p className='text-destructive text-xs' data-testid='preview-runs-error'>
        {isPreviewRunsDisabled(error)
          ? "Preview runs aren't enabled on this deployment."
          : error.message}
      </p>
    );
  }
  if (!data || data.length === 0) {
    return <p className='text-muted-foreground text-xs'>No dry-runs yet.</p>;
  }

  return (
    <div className='flex flex-col gap-1.5' data-testid='preview-runs-list'>
      {groupPreviewRuns(data).map(({ run, children }) => (
        <PreviewRunRow key={run.run_id} workspaceId={workspaceId} run={run} childRuns={children} />
      ))}
    </div>
  );
}
