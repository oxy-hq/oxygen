import { Hammer, Loader2, Maximize2, RotateCcw, Sparkles, X } from "lucide-react";
import { useCallback, useMemo, useState } from "react";
import { toast } from "sonner";
import ErrorAlert from "@/components/ui/ErrorAlert";
import { Button } from "@/components/ui/shadcn/button";
import { Dialog, DialogContent } from "@/components/ui/shadcn/dialog";
import useRevertBuilderFileChanges from "@/hooks/api/analytics/useRevertBuilderFileChanges";
import type { BuilderFileChange } from "@/hooks/useBuilderActivity";
import { useBuilderActivity } from "@/hooks/useBuilderActivity";

import AnalyticsReasoningTrace from "../AnalyticsReasoningTrace";
import ChangeVisualization from "../BuilderActivityPanel/ChangeVisualization";
import { useBuilderDelegationEvents } from "./useBuilderDelegationEvents";

interface BuilderDelegationPanelProps {
  childRunId: string;
  projectId: string;
  onClose: () => void;
}

/** A file change reverted via the audit event carries this description prefix. */
const REVERTED_PREFIX = "Reverted:";

export default function BuilderDelegationPanel({
  childRunId,
  projectId,
  onClose
}: BuilderDelegationPanelProps) {
  const {
    events,
    isStreaming,
    error: streamError
  } = useBuilderDelegationEvents(projectId, childRunId, true);
  const [fullscreen, setFullscreen] = useState<BuilderFileChange | null>(null);
  // Optimistic: the child run's SSE is one-shot, so a revert's audit event
  // won't re-stream — track locally for immediate feedback (a reload
  // re-derives the reverted state from the persisted "Reverted:" event).
  const [revertedPaths, setRevertedPaths] = useState<Set<string>>(new Set());

  const revertMutation = useRevertBuilderFileChanges(projectId);

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

  const isReverted = useCallback(
    (c: BuilderFileChange) =>
      revertedPaths.has(c.filePath) || c.description.startsWith(REVERTED_PREFIX),
    [revertedPaths]
  );

  const revertablePaths = useMemo(
    () => fileChanges.filter((c) => !isReverted(c)).map((c) => c.filePath),
    [fileChanges, isReverted]
  );

  const doRevert = useCallback(
    (filePaths: string[]) => {
      if (filePaths.length === 0) return;
      revertMutation.mutate(
        { runId: childRunId, filePaths },
        {
          onSuccess: (res) => {
            setRevertedPaths((prev) => {
              const next = new Set(prev);
              for (const r of res.reverted) next.add(r.file_path);
              return next;
            });
            toast.success(
              res.reverted.length === 1
                ? "Change reverted"
                : `Reverted ${res.reverted.length} files`
            );
          },
          onError: (err: unknown) => {
            const msg =
              err && typeof err === "object" && "response" in err
                ? ((err as { response?: { data?: { error?: string } } }).response?.data?.error ??
                  "Failed to revert change")
                : "Failed to revert change";
            toast.error(msg);
          }
        }
      );
    },
    [revertMutation, childRunId]
  );

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
          {revertablePaths.length > 1 && (
            <Button
              variant='outline'
              size='sm'
              disabled={revertMutation.isPending}
              onClick={() => doRevert(revertablePaths)}
            >
              {revertMutation.isPending ? (
                <Loader2 className='h-3.5 w-3.5 animate-spin' />
              ) : (
                <RotateCcw className='h-3.5 w-3.5' />
              )}
              Revert all
            </Button>
          )}
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
              const reverted = isReverted(change);
              return (
                <div key={change.id} className='rounded-lg border'>
                  <div className='flex items-center justify-between gap-2 border-b px-3 py-2'>
                    <span className='min-w-0 truncate font-medium text-xs'>{change.filePath}</span>
                    <div className='flex shrink-0 items-center gap-1'>
                      {reverted ? (
                        <span className='rounded bg-muted px-2 py-0.5 text-[11px] text-muted-foreground'>
                          Reverted
                        </span>
                      ) : (
                        <Button
                          variant='ghost'
                          size='sm'
                          disabled={revertMutation.isPending}
                          onClick={() => doRevert([change.filePath])}
                        >
                          {revertMutation.isPending ? (
                            <Loader2 className='h-3.5 w-3.5 animate-spin' />
                          ) : (
                            <RotateCcw className='h-3.5 w-3.5' />
                          )}
                          Revert
                        </Button>
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
