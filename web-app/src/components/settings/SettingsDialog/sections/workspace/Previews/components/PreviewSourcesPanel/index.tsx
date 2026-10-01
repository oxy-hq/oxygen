import { useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { usePreviewSources } from "@/hooks/api/workspaces/usePreviews";
import PreviewSourceForm from "./PreviewSourceForm";
import PreviewSourceRow from "./PreviewSourceRow";

/**
 * Sandbox sources for rotate-on-use pipelines (QuickBooks: one sandbox
 * company per customer). Workspace-wide, not per-branch or per-preview — a
 * pipeline's sandbox credentials serve every branch's Airway sample of it,
 * the same way its production credentials serve every branch's live
 * pipeline. An `.airway.yml`'s sample form (below, per branch) refuses with
 * `sandbox_required` until its pipeline has a row here.
 */
export default function PreviewSourcesPanel({ workspaceId }: { workspaceId: string }) {
  const { data, isLoading, error } = usePreviewSources(workspaceId);
  const [adding, setAdding] = useState(false);

  return (
    <div className='flex flex-col gap-2' data-testid='preview-sources-panel'>
      <div className='flex items-center justify-between'>
        <h3 className='font-medium text-sm'>Sandbox sources</h3>
        {!adding && (
          <Button
            variant='outline'
            size='sm'
            onClick={() => setAdding(true)}
            data-testid='preview-sources-add-toggle'
          >
            Register a source
          </Button>
        )}
      </div>
      <p className='text-muted-foreground text-xs'>
        A rotate-on-use pipeline (QuickBooks) needs its sandbox company registered here before an
        Airway sample of it can run.
      </p>
      {adding && <PreviewSourceForm workspaceId={workspaceId} onDone={() => setAdding(false)} />}
      {isLoading && <Spinner className='size-4 text-muted-foreground' />}
      {error && (
        <p className='text-destructive text-xs' data-testid='preview-sources-error'>
          {error.message}
        </p>
      )}
      {data && data.length === 0 && !adding && (
        <p className='text-muted-foreground text-xs'>No sandbox sources registered yet.</p>
      )}
      {data && data.length > 0 && (
        <div className='flex flex-col gap-1.5' data-testid='preview-sources-list'>
          {data.map((source) => (
            <PreviewSourceRow key={source.pipeline} workspaceId={workspaceId} source={source} />
          ))}
        </div>
      )}
    </div>
  );
}
