import { Check, Loader2, Pencil, X } from "lucide-react";
import type React from "react";
import { useEffect, useRef, useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/shadcn/button";
import { Input } from "@/components/ui/shadcn/input";
import usePatchEdgeBoxCohort from "@/hooks/api/cameras/usePatchEdgeBoxCohort";
import { cn } from "@/libs/shadcn/utils";

/**
 * Inline-editable cohort cell for the EdgeBoxesTable.
 *
 * Two states:
 *   - **read**: shows the cohort text + a pencil icon on hover. Click
 *     anywhere on the cell to enter edit mode.
 *   - **edit**: input box pre-focused with the current value selected,
 *     ✓ to commit (Enter) and ✗ to cancel (Escape).
 *
 * The mutation invalidates both `edgeBoxes` and `fleetMembers` so
 * the rollout wizard's cohort dropdown picks up the new label without
 * a page refresh.
 */
const CohortCell: React.FC<{
  workspaceId: string | undefined;
  boxId: string;
  cohort: string;
}> = ({ workspaceId, boxId, cohort }) => {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(cohort);
  const inputRef = useRef<HTMLInputElement>(null);
  const mut = usePatchEdgeBoxCohort(workspaceId);

  // Reset the draft when the upstream value changes — covers the
  // case where another operator edits the same row while we're
  // reading it but before we enter edit mode.
  useEffect(() => {
    if (!editing) setDraft(cohort);
  }, [cohort, editing]);

  // Auto-focus + select-all on enter so the operator can immediately
  // type-over without hunting for the input.
  useEffect(() => {
    if (editing) {
      inputRef.current?.focus();
      inputRef.current?.select();
    }
  }, [editing]);

  const commit = async () => {
    const trimmed = draft.trim();
    if (!trimmed) {
      toast.error("Cohort can't be empty — use 'default' for the catch-all group.");
      return;
    }
    if (trimmed === cohort) {
      setEditing(false);
      return;
    }
    try {
      await mut.mutateAsync({ boxId, cohort: trimmed });
      setEditing(false);
    } catch (e) {
      toast.error(e instanceof Error ? e.message : "Failed to update cohort.");
    }
  };

  const cancel = () => {
    setDraft(cohort);
    setEditing(false);
  };

  if (editing) {
    return (
      <div className='flex items-center gap-1'>
        <Input
          ref={inputRef}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              void commit();
            } else if (e.key === "Escape") {
              e.preventDefault();
              cancel();
            }
          }}
          maxLength={64}
          className='h-7 max-w-32 px-2 py-1 text-xs'
          disabled={mut.isPending}
          data-testid={`cohort-input-${boxId}`}
        />
        <Button
          size='icon'
          variant='ghost'
          className='h-6 w-6'
          onClick={() => void commit()}
          disabled={mut.isPending}
          aria-label='Save cohort'
        >
          {mut.isPending ? (
            <Loader2 className='h-3 w-3 animate-spin' />
          ) : (
            <Check className='h-3 w-3' />
          )}
        </Button>
        <Button
          size='icon'
          variant='ghost'
          className='h-6 w-6'
          onClick={cancel}
          disabled={mut.isPending}
          aria-label='Cancel cohort edit'
        >
          <X className='h-3 w-3' />
        </Button>
      </div>
    );
  }

  return (
    <button
      type='button'
      onClick={() => setEditing(true)}
      className={cn(
        "group inline-flex items-center gap-1 px-1 py-0.5 text-left text-muted-foreground",
        "hover:bg-muted hover:text-foreground"
      )}
      aria-label={`Edit cohort (currently ${cohort})`}
      data-testid={`cohort-cell-${boxId}`}
    >
      <span>{cohort}</span>
      <Pencil className='h-3 w-3 opacity-0 group-hover:opacity-60' />
    </button>
  );
};

export default CohortCell;
