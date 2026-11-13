import { Hammer, Loader2, Maximize2, Sparkles, X } from "lucide-react";
import { useCallback, useMemo, useState } from "react";
import ErrorAlert from "@/components/ui/ErrorAlert";
import { Dialog, DialogContent } from "@/components/ui/shadcn/dialog";
import useAppsFirstExperience from "@/hooks/useAppsFirstExperience";
import type { BuilderFileChange } from "@/hooks/useBuilderActivity";
import { useBuilderActivity } from "@/hooks/useBuilderActivity";
import { objectNameFromPath } from "@/libs/utils/objectName";

import AnalyticsReasoningTrace from "../AnalyticsReasoningTrace";
import ChangeVisualization from "../BuilderActivityPanel/ChangeVisualization";
import { useBuilderDelegationEvents } from "./useBuilderDelegationEvents";

interface BuilderDelegationPanelProps {
  childRunId: string;
  projectId: string;
  onClose: () => void;
}

/**
 * A file change reverted via the audit event carries this description prefix.
 * The panel only shows a past run: reverting went with the Builder.
 */
const REVERTED_PREFIX = "Reverted:";

export default function BuilderDelegationPanel({
  childRunId,
  projectId,
  onClose
}: BuilderDelegationPanelProps) {
  // Apps-first: what changed by plain name, never by file path.
  const plainNames = useAppsFirstExperience();
  const {
    events,
    isStreaming,
    error: streamError
  } = useBuilderDelegationEvents(projectId, childRunId, true);
  const [fullscreen, setFullscreen] = useState<BuilderFileChange | null>(null);
  // Empty — delegated builder runs auto-accept; no manual accept/reject here.
  const changeDecisions = useMemo(() => new Map<number, "accepted" | "rejected">(), []);
  const activityItems = useBuilderActivity(events, changeDecisions);

  // Builder child runs don't nest further delegations — no-op is sufficient.
  const onSelectArtifact = useCallback(() => {}, []);

  // Latest change per file (re-edits collapse to the most recent state),
  // in first-seen order.
  const fileChanges = useMemo(() => {
    const byPath = new Map<string, BuilderFileChange>();
    for (const item of activityItems) {
      if (item.kind === "file_changed") byPath.set(item.filePath, item);
    }
    return Array.from(byPath.values());
  }, [activityItems]);

  const isReverted = (c: BuilderFileChange) => c.description.startsWith(REVERTED_PREFIX);

  // A failed stream is not a finished run: what is shown may be missing or partial.
  const subtitle = isStreaming
    ? "Working on semantic model changes…"
    : streamError
      ? "Activity unavailable"
      : "Completed";

  return (
    <div className='flex h-full flex-col border-l bg-background'>
      {/* Header */}
      <div className='flex shrink-0 items-center justify-between border-b px-4 py-3'>
        <div className='flex min-w-0 items-center gap-2'>
          {isStreaming ? (
            <Loader2 className='h-4 w-4 shrink-0 animate-spin text-primary' />
          ) : (
            <Hammer className='h-4 w-4 shrink-0 text-primary' />
          )}
          <div className='min-w-0'>
            <h3 className='font-semibold text-sm'>Builder Agent</h3>
            <p className='mt-0.5 text-[11px] text-muted-foreground'>{subtitle}</p>
          </div>
        </div>
        <div className='ml-2 flex items-center gap-1'>
          <button
            type='button'
            onClick={onClose}
            className='rounded p-1 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground'
            aria-label='Close panel'
          >
            <X className='h-4 w-4' />
          </button>
        </div>
      </div>

      {/* Reasoning trace + per-file change cards */}
      <div className='flex min-h-0 flex-1 flex-col overflow-y-auto p-4'>
        {streamError && (
          <ErrorAlert
            className='mb-4'
            title="Failed to load the Builder Agent's activity"
            message='The steps and changes shown may be incomplete. Close and reopen this panel to try again.'
          />
        )}

        {(events.length > 0 || isStreaming) && (
          <div className='mb-4'>
            <AnalyticsReasoningTrace
              events={events}
              isRunning={isStreaming}
              onSelectArtifact={onSelectArtifact}
            />
          </div>
        )}

        {fileChanges.length > 0 ? (
          <div className='space-y-4'>
            {fileChanges.map((change) => {
              return (
                <div key={change.id} className='rounded-lg border'>
                  <div className='flex items-center justify-between gap-2 border-b px-3 py-2'>
                    <span className='min-w-0 truncate font-medium text-xs'>
                      {plainNames ? objectNameFromPath(change.filePath) : change.filePath}
                    </span>
                    <div className='flex shrink-0 items-center gap-1'>
                      {isReverted(change) && (
                        <span className='rounded bg-muted px-2 py-0.5 text-[11px] text-muted-foreground'>
                          Reverted
                        </span>
                      )}
                      <button
                        type='button'
                        onClick={() => setFullscreen(change)}
                        className='rounded p-1 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground'
                        aria-label='Fullscreen'
                      >
                        <Maximize2 className='h-3.5 w-3.5' />
                      </button>
                    </div>
                  </div>
                  <div className='p-3'>
                    <ChangeVisualization change={change} />
                  </div>
                </div>
              );
            })}
          </div>
        ) : (
          !isStreaming &&
          !streamError && (
            <div className='flex flex-1 flex-col items-center justify-center gap-3 p-6 text-center'>
              <div className='rounded-full bg-muted p-3'>
                <Sparkles className='h-5 w-5 text-muted-foreground' />
              </div>
              <p className='text-muted-foreground text-sm'>No changes proposed.</p>
            </div>
          )
        )}

        {/* Fullscreen dialog — graph only, no header text */}
        <Dialog open={fullscreen !== null} onOpenChange={(open) => !open && setFullscreen(null)}>
          <DialogContent
            className='flex h-[80vh] w-[80vw] max-w-[80vw] flex-col gap-0 p-4'
            showCloseButton={false}
          >
            <div className='relative min-h-0 flex-1 overflow-hidden'>
              {fullscreen && <ChangeVisualization change={fullscreen} />}
              <button
                type='button'
                onClick={() => setFullscreen(null)}
                className='absolute top-2 right-2 rounded p-1 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground'
                aria-label='Close fullscreen'
              >
                <X className='h-4 w-4' />
              </button>
            </div>
          </DialogContent>
        </Dialog>
      </div>
    </div>
  );
}
