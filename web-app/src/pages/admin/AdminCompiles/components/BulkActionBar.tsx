import { Hammer, Loader2, Rocket, X } from "lucide-react";
import { toast } from "sonner";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger
} from "@/components/ui/shadcn/alert-dialog";
import { Button } from "@/components/ui/shadcn/button";
import { useBatchPromoteCompiles, useBatchRunCompiles } from "@/hooks/api/compiles";

/** Compose a "Recompiled 4, 1 failed" summary from a batch response. */
function summarize(ok: number, errors: number, verb: string): string {
  if (errors === 0) return `${verb} ${ok}.`;
  if (ok === 0) return `${verb} 0, ${errors} failed.`;
  return `${verb} ${ok}, ${errors} failed.`;
}

/**
 * Sticky bulk-action bar. Appears only when rows are selected. The action
 * set depends on the active view:
 *   - workspaces → "Recompile selected" (batch run, promote:true)
 *   - revisions  → "Promote selected" (batch promote)
 * Both sit behind an AlertDialog confirm; the batch response is folded
 * into a single success/error toast.
 */
export const BulkActionBar = ({
  mode,
  selectedIds,
  onClear
}: {
  mode: "workspace" | "revisions";
  selectedIds: string[];
  onClear: () => void;
}) => {
  const batchRun = useBatchRunCompiles();
  const batchPromote = useBatchPromoteCompiles();
  const count = selectedIds.length;
  if (count === 0) return null;

  const noun = mode === "workspace" ? "workspace" : "revision";
  const isPending = batchRun.isPending || batchPromote.isPending;

  const runRecompile = () =>
    batchRun.mutate(
      { workspaceIds: selectedIds, promote: true },
      {
        onSuccess: (res) => {
          const failed = res.results.filter((r) => r.error).length;
          toast.success(summarize(res.enqueued, failed, "Recompiled"));
          if (failed === 0) onClear();
        },
        onError: (e) => toast.error(e instanceof Error ? e.message : "Recompile failed")
      }
    );

  const runPromote = () =>
    batchPromote.mutate(selectedIds, {
      onSuccess: (res) => {
        const failed = res.results.filter((r) => r.error).length;
        toast.success(summarize(res.promoted, failed, "Promoted"));
        if (failed === 0) onClear();
      },
      onError: (e) => toast.error(e instanceof Error ? e.message : "Promote failed")
    });

  return (
    <div className='fixed bottom-4 left-1/2 z-20 flex -translate-x-1/2 items-center gap-3 border border-border bg-card px-3 py-2 shadow-lg'>
      <span className='font-medium text-xs tabular-nums'>
        {count} {noun}
        {count === 1 ? "" : "s"} selected
      </span>
      <div className='h-4 w-px bg-border' aria-hidden />
      <AlertDialog>
        <AlertDialogTrigger asChild>
          <Button size='sm' className='h-7 gap-1.5' disabled={isPending}>
            {isPending ? (
              <Loader2 className='size-3.5 animate-spin' />
            ) : mode === "workspace" ? (
              <Hammer className='size-3.5' />
            ) : (
              <Rocket className='size-3.5' />
            )}
            {mode === "workspace" ? "Recompile selected" : "Promote selected"}
          </Button>
        </AlertDialogTrigger>
        <AlertDialogContent className={DESIGN_REFERENCE}>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {mode === "workspace"
                ? `Recompile ${count} workspace${count === 1 ? "" : "s"}?`
                : `Promote ${count} revision${count === 1 ? "" : "s"}?`}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {mode === "workspace"
                ? "Enqueues a promoting compile for each selected workspace. The runtime serves each new revision once it reaches ready."
                : "Repoints each selected revision's workspace at that revision. This is a manual rollback — the runtime serves these definitions immediately."}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction onClick={mode === "workspace" ? runRecompile : runPromote}>
              {mode === "workspace" ? "Recompile" : "Promote"}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
      <Button size='sm' variant='ghost' onClick={onClear} className='h-7 gap-1 px-2 text-xs'>
        <X className='size-3.5' />
        Clear
      </Button>
    </div>
  );
};
