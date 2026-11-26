import { Loader2 } from "lucide-react";
import type React from "react";
import { useEffect, useState } from "react";
import { toast } from "sonner";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import { Button } from "@/components/ui/shadcn/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle
} from "@/components/ui/shadcn/dialog";
import { Input } from "@/components/ui/shadcn/input";
import { Label } from "@/components/ui/shadcn/label";
import useBulkPatchEdgeBoxTarget from "@/hooks/api/cameras/useBulkPatchEdgeBoxTarget";
import usePatchEdgeBoxTarget from "@/hooks/api/cameras/usePatchEdgeBoxTarget";
import type { EdgeBoxSummary } from "@/services/api/cameras";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";

/**
 * Two modes:
 *   - single: one edge box, shows its currently-running tag for
 *     reference, edits its target.
 *   - bulk: a list of selected edge boxes, sets one target across
 *     all of them in one atomic backend call.
 *
 * Mode is inferred from `boxes.length`: 1 → single, >1 → bulk. The
 * caller doesn't pick — they just pass whatever's selected.
 */
type Props = {
  boxes: EdgeBoxSummary[];
  open: boolean;
  onOpenChange: (open: boolean) => void;
};

const SetTargetImageDialog: React.FC<Props> = ({ boxes, open, onOpenChange }) => {
  const { workspace } = useCurrentWorkspace();
  const single = usePatchEdgeBoxTarget(workspace?.id);
  const bulk = useBulkPatchEdgeBoxTarget(workspace?.id);
  const [tag, setTag] = useState("");

  const isBulk = boxes.length > 1;
  const isPending = single.isPending || bulk.isPending;

  // Reset on open. In bulk mode we don't pre-populate the field —
  // operators expect a clean slate to type the new tag, not a
  // mixed-state "(varies)" placeholder we'd have to compute.
  useEffect(() => {
    if (!open) return;
    if (boxes.length === 1) {
      setTag(boxes[0].target_image_tag ?? "");
    } else {
      setTag("");
    }
  }, [open, boxes]);

  if (boxes.length === 0) return null;

  const onSubmit = async () => {
    if (!workspace?.id) return;
    const trimmed = tag.trim();
    const target = trimmed === "" ? null : trimmed;
    try {
      if (isBulk) {
        const res = await bulk.mutateAsync({
          boxIds: boxes.map((b) => b.id),
          targetImageTag: target
        });
        toast.success(
          target === null
            ? `Cleared target on ${res.updated_count} boxes.`
            : `Set ${trimmed} on ${res.updated_count} boxes.`
        );
      } else {
        await single.mutateAsync({
          boxId: boxes[0].id,
          targetImageTag: target
        });
        toast.success(
          target === null
            ? "Target image cleared — automated updates disabled."
            : `Target image set to ${trimmed}.`
        );
      }
      onOpenChange(false);
    } catch (err) {
      const msg = err instanceof Error ? err.message : "Could not set target";
      toast.error(msg);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className={DESIGN_REFERENCE}>
        <DialogHeader>
          <DialogTitle>
            {isBulk ? `Set target on ${boxes.length} boxes` : "Set target image"}
          </DialogTitle>
          <DialogDescription>
            {isBulk
              ? "The same target image will be applied to every selected box. Each device pulls + restarts independently within ~5 minutes of its next poll."
              : "The device polls for this every ~5 minutes and pulls + restarts the worker when it differs from what's running. Leave blank to clear (disable automated updates)."}
          </DialogDescription>
        </DialogHeader>

        <div className='flex flex-col gap-3'>
          {!isBulk && (
            <div className='flex flex-col gap-1'>
              <Label className='text-muted-foreground text-xs uppercase tracking-wide'>
                Currently running
              </Label>
              <code className='border border-border bg-muted/50 px-2 py-1 text-sm'>
                {boxes[0].current_image_tag ?? "(not reported yet)"}
              </code>
            </div>
          )}

          {isBulk && (
            <div className='flex flex-col gap-1'>
              <Label className='text-muted-foreground text-xs uppercase tracking-wide'>
                Selected boxes
              </Label>
              <ul className='max-h-32 overflow-auto border border-border bg-muted/30 p-2 text-xs'>
                {boxes.map((b) => (
                  <li key={b.id} className='truncate'>
                    {b.hardware_model}{" "}
                    <span className='text-muted-foreground'>· {b.site_name}</span>
                  </li>
                ))}
              </ul>
            </div>
          )}

          <div className='flex flex-col gap-1.5'>
            <Label htmlFor='target-tag'>Target image tag</Label>
            <Input
              id='target-tag'
              value={tag}
              onChange={(e) => setTag(e.target.value)}
              placeholder='ghcr.io/oxy/edge:3.4.2'
              autoComplete='off'
              spellCheck={false}
              className='font-mono'
            />
            <span className='text-muted-foreground text-xs'>
              Must be a valid `docker pull` reference. The agent reverts on a failed pull.
            </span>
          </div>
        </div>

        <DialogFooter>
          <Button variant='outline' onClick={() => onOpenChange(false)} disabled={isPending}>
            Cancel
          </Button>
          <Button onClick={onSubmit} disabled={isPending}>
            {isPending ? (
              <>
                <Loader2 className='mr-2 h-4 w-4 animate-spin' />
                Saving…
              </>
            ) : isBulk ? (
              `Save target on ${boxes.length} boxes`
            ) : (
              "Save target"
            )}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};

export default SetTargetImageDialog;
