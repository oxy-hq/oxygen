import { Loader2, RotateCcw } from "lucide-react";
import type React from "react";
import { useState } from "react";
import { toast } from "sonner";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle
} from "@/components/ui/shadcn/alert-dialog";
import { Button } from "@/components/ui/shadcn/button";
import { useResetSchema } from "@/hooks/api/airway/useAirway";

/**
 * "Reset schema" — drops the pipeline's destination tables and clears its stored
 * schema + incremental cursor, so a later run re-infers a fresh schema. The
 * escape hatch for a pipeline stuck on a wrong schema: airway migration is
 * additive (it never retypes or drops a column), so a bad schema can't self-heal.
 *
 * Destructive — the dropped tables' data is gone until a backfill repopulates
 * them — so it's gated behind a confirm dialog. Airhouse-only: the backend
 * rejects other destinations, surfaced here as an error toast.
 *
 * **It is no longer the only reset on screen.** `ResetCursorsButton` rewinds a
 * pipeline's cursors while every row stays put, and it sits to the left of this
 * one carrying the `outline` weight this button used to have. That reordering
 * is the fix, not decoration: while this was the only visible reset, an operator
 * who needed to re-pull one resource by 180 days was offered the button that
 * drops every destination table — including 3.7M rows of an append-only
 * resource whose source only ever serves today's snapshot. Hence `ghost` and a
 * muted label here, and the cross-link in the dialog below. Visible and
 * findable, no longer the heaviest thing on the shelf.
 */
const ResetSchemaButton: React.FC<{ pipelineRef: string }> = ({ pipelineRef }) => {
  const [open, setOpen] = useState(false);
  const reset = useResetSchema();

  return (
    <>
      <Button
        size='sm'
        variant='ghost'
        onClick={() => setOpen(true)}
        disabled={reset.isPending}
        aria-label='Reset schema'
        data-testid='airway-reset-schema-button'
        className='text-muted-foreground hover:text-destructive'
      >
        {reset.isPending ? (
          <Loader2 className='h-4 w-4 animate-spin' />
        ) : (
          <RotateCcw className='h-4 w-4' />
        )}
        Reset schema
      </Button>
      <AlertDialog open={open} onOpenChange={setOpen}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Reset this pipeline’s schema?</AlertDialogTitle>
            <AlertDialogDescription asChild>
              <div className='space-y-2'>
                <p>
                  This{" "}
                  <span className='font-medium text-destructive'>drops the destination tables</span>{" "}
                  for <span className='font-mono text-xs'>{pipelineRef}</span> and clears its stored
                  schema + incremental cursor.
                </p>
                {/* The highest-value line in this dialog, and the reason the
                    cursor rewind exists. Someone who only needs an earlier
                    starting point reaches for this button because it is the one
                    that sounds like a reset — and pays for a rewind with every
                    row the pipeline has ever landed. Do not cut this as copy
                    polish; it is the fix. */}
                <p
                  data-testid='airway-reset-schema-rewind-hint'
                  className='rounded-md border border-border bg-muted/50 p-2 text-xs'
                >
                  Only need to re-pull from an earlier date?{" "}
                  <span className='font-medium text-foreground'>Use Rewind cursors</span> — it moves
                  the incremental position and keeps every row.
                </p>
                <p className='text-xs'>
                  Use this when the pipeline is stuck on a wrong schema — airway only ever adds
                  columns, so it can’t fix a bad type on its own. The tables’ data is dropped and
                  the next run re-infers a fresh schema.{" "}
                  <span className='font-medium'>Run a backfill afterward</span> to repopulate.
                  Airhouse destinations only.
                </p>
              </div>
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              className='bg-destructive text-destructive-foreground hover:bg-destructive/90'
              onClick={async () => {
                setOpen(false);
                try {
                  const { dropped_tables } = await reset.mutateAsync({
                    pipeline_ref: pipelineRef
                  });
                  toast.success(
                    dropped_tables.length
                      ? `Schema reset — dropped ${dropped_tables.length} table${
                          dropped_tables.length === 1 ? "" : "s"
                        }. Backfill to repopulate.`
                      : "Schema reset — nothing was provisioned yet."
                  );
                } catch (e) {
                  toast.error(e instanceof Error ? `Reset failed: ${e.message}` : "Reset failed");
                }
              }}
            >
              Reset schema
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
};

export default ResetSchemaButton;
