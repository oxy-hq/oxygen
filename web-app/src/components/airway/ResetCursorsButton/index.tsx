import { History, Loader2 } from "lucide-react";
import type React from "react";
import { useCallback, useState } from "react";
import { toast } from "sonner";

import RefusalPanel from "@/components/airway/ResetCursorsButton/components/RefusalPanel";
import ResourcePicker from "@/components/airway/ResetCursorsButton/components/ResourcePicker";
import { coversEvery } from "@/components/airway/ResetCursorsButton/selection";
import { Button } from "@/components/ui/shadcn/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle
} from "@/components/ui/shadcn/dialog";
import { useAirwayResourceCursors, useResetCursors } from "@/hooks/api/airway/useAirway";
import { apiErrorMessage } from "@/libs/apiError";

/**
 * "Rewind cursors" — moves a pipeline's incremental cursors back so the next run
 * re-pulls those resources from their `default_start`. **Nothing is dropped.**
 *
 * The safe half of a pair. Its sibling, `ResetSchemaButton`, drops every
 * destination table; until this existed that was the only visible reset, so an
 * operator who wanted to re-pull one resource by 180 days was offered a button
 * that would have destroyed 3.7M rows of an unrelated append-only resource to
 * do it. That is why this one carries the visual weight and the other does not,
 * and why both dialogs name their consequence rather than leaving it to the
 * verb.
 *
 * A `Dialog`, not an `AlertDialog`: the refusal path keeps the dialog open and
 * grows content inside it, which is not the single-question shape an alert
 * dialog is for.
 */
const ResetCursorsButton: React.FC<{ pipelineRef: string }> = ({ pipelineRef }) => {
  const [open, setOpen] = useState(false);
  const [selected, setSelected] = useState<string[]>([]);
  const [refusal, setRefusal] = useState<string[] | null>(null);
  const [overrideRevealed, setOverrideRevealed] = useState(false);
  /** Something (a run, or another reset) holds the pipeline lease. Not a refusal: nothing to override. */
  const [running, setRunning] = useState<string | null>(null);

  const {
    data: resources,
    isLoading,
    isError,
    error
  } = useAirwayResourceCursors(pipelineRef, open);
  const reset = useResetCursors();

  /**
   * A refusal answers one specific scope. Change the scope and the answer no
   * longer applies — so both the reasons and any revealed override go with it.
   * A `force` left armed across a selection change is an override of reasons
   * that were never about this request.
   */
  const changeSelection = useCallback((next: string[]) => {
    setSelected(next);
    setRefusal(null);
    setOverrideRevealed(false);
    setRunning(null);
  }, []);

  const reopen = (next: boolean) => {
    // No dismissing while a rewind is in flight. A refusal that lands in a
    // closed dialog is written to state nothing renders, then wiped by the
    // next open — the server's reasons swallowed whole, which is the failure
    // this component exists to prevent. Every dismiss path (Cancel, Esc,
    // outside click, the close button) comes through here.
    if (!next && reset.isPending) return;
    setOpen(next);
    if (!next) return;
    // Every open starts from nothing selected and no standing refusal.
    setSelected([]);
    setRefusal(null);
    setOverrideRevealed(false);
    setRunning(null);
  };

  /**
   * Whether the selection covers every held resource — for the label only.
   * What is sent is always the selection itself, never `[]`: the server
   * resolves `[]` against the cursors held when the request lands, which can
   * include one that appeared after this list loaded. A list naming every
   * held cursor is judged exactly as `[]` is, so sending it costs no refusal.
   *
   * A set predicate, never a count. `held` is live query data (refetched on
   * focus, invalidated by a schema reset), so it can shrink under an open
   * dialog: with `held = [a, b]` and only `a` ticked, a refetch returning
   * `[b]` made the counts equal, and a partial selection went out as "every
   * resource" — clearing `b`, which the operator never picked.
   */
  const held = resources ?? [];
  const isEveryResource = coversEvery(held, selected);

  const scopeLabel = isEveryResource
    ? "every resource"
    : selected.length === 1
      ? selected[0]
      : `${selected.length} resources`;

  const submit = async (force: boolean) => {
    try {
      const outcome = await reset.mutateAsync({
        pipeline_ref: pipelineRef,
        resources: selected,
        force
      });

      if (outcome.kind === "pipeline_running") {
        // Kept in the dialog like a refusal, but with no override: `force`
        // cannot get past a held lease, so offering it would be a lie. The
        // server's message says who holds it; the confirm stays live —
        // retrying once the holder lets go is the answer.
        setRunning(outcome.message);
        return;
      }
      setRunning(null);

      if (outcome.kind === "refused") {
        // Keep the dialog open: the reasons are the point, and a toast that
        // disappears is not somewhere an operator can read ninety words.
        setRefusal(outcome.reasons);
        setOverrideRevealed(false);
        return;
      }

      setOpen(false);
      const n = outcome.cleared.length;
      toast.success(
        n === 0
          ? "No cursors were held, so nothing was rewound."
          : `Rewound ${n} cursor${n === 1 ? "" : "s"}. No data was dropped — run the pipeline to re-pull.`
      );
      if (outcome.not_held.length > 0) {
        // Only reachable if the list went stale between opening and submitting,
        // but saying nothing would let a partial reset read as a whole one.
        toast.warning(`No cursor was held for: ${outcome.not_held.join(", ")}`);
      }
    } catch (e) {
      toast.error(apiErrorMessage(e, "Couldn’t rewind the cursors."));
    }
  };

  return (
    <>
      <Button
        size='sm'
        variant='outline'
        onClick={() => reopen(true)}
        aria-label='Rewind cursors'
        data-testid='airway-rewind-cursors-button'
      >
        <History className='h-4 w-4' />
        Rewind cursors
      </Button>

      <Dialog open={open} onOpenChange={reopen}>
        <DialogContent className='max-w-lg'>
          <DialogHeader>
            <DialogTitle>Rewind cursors — nothing is dropped</DialogTitle>
            <DialogDescription asChild>
              <div className='space-y-2'>
                <p>
                  The next run re-pulls the resources you pick from their{" "}
                  <span className='font-mono text-xs'>default_start</span>.{" "}
                  <span className='font-medium text-foreground'>Every landed row stays.</span>
                </p>
                <p className='text-xs'>
                  Not to be confused with <span className='font-medium'>Reset schema</span>, which
                  drops every destination table. This moves a position; that destroys data.
                </p>
              </div>
            </DialogDescription>
          </DialogHeader>

          <div className='space-y-4'>
            {isLoading ? (
              <p className='text-muted-foreground text-xs'>Loading this pipeline’s cursors…</p>
            ) : isError ? (
              <p className='text-destructive text-xs'>
                {apiErrorMessage(error, "Couldn’t read this pipeline’s cursors.")}
              </p>
            ) : (
              <ResourcePicker
                resources={held}
                selected={selected}
                onToggle={(r) =>
                  changeSelection(
                    selected.includes(r) ? selected.filter((s) => s !== r) : [...selected, r]
                  )
                }
                onSelectEvery={(every) => changeSelection(every ? held : [])}
                disabled={reset.isPending}
              />
            )}

            {running && (
              <p
                role='alert'
                data-testid='airway-cursor-pipeline-running'
                className='rounded-md border border-border bg-muted/50 p-2 text-xs'
              >
                {running}
              </p>
            )}

            {refusal && (
              <RefusalPanel
                reasons={refusal}
                scopeLabel={scopeLabel}
                overrideRevealed={overrideRevealed}
                onRevealOverride={() => setOverrideRevealed(true)}
                onForce={() => submit(true)}
                forcing={reset.isPending}
              />
            )}
          </div>

          <DialogFooter>
            <Button
              variant='outline'
              size='sm'
              onClick={() => reopen(false)}
              disabled={reset.isPending}
            >
              Cancel
            </Button>
            <Button
              size='sm'
              onClick={() => submit(false)}
              // Nothing selected is not a request the route can serve safely:
              // it reads an empty list as *every* resource. Disabled rather
              // than defaulted, so "all" is never an accident.
              disabled={selected.length === 0 || reset.isPending || refusal !== null}
              data-testid='airway-rewind-cursors-confirm'
            >
              {reset.isPending && <Loader2 className='h-4 w-4 animate-spin' />}
              Rewind {selected.length > 0 ? scopeLabel : "selected"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
};

export default ResetCursorsButton;
