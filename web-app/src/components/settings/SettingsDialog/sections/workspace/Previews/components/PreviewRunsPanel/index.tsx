import PreviewRunForm from "./PreviewRunForm";
import PreviewRunsList from "./PreviewRunsList";
import PreviewSampleRunForm from "./PreviewSampleRunForm";

/**
 * A preview's held-run surface: submit a procedure to dry-run against the
 * branch's staging revision (every write held, reported as "would have
 * written"), submit a bounded Airway sample of a pipeline into the preview,
 * and the runs already tried (of either kind).
 */
export default function PreviewRunsPanel({
  workspaceId,
  branch
}: {
  workspaceId: string;
  branch: string;
}) {
  return (
    <div className='flex flex-col gap-3' data-testid='preview-runs-panel'>
      <PreviewRunForm workspaceId={workspaceId} branch={branch} />
      <PreviewSampleRunForm workspaceId={workspaceId} branch={branch} />
      <PreviewRunsList workspaceId={workspaceId} branch={branch} />
    </div>
  );
}
