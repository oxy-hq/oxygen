import { Copy, MoreHorizontal, Pencil } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/shadcn/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger
} from "@/components/ui/shadcn/dropdown-menu";
import { useExperimentDesign, useSaveDesign } from "@/hooks/api/useExperiments";
import { useRole } from "@/hooks/useRole";
import type { ExperimentDesignInput, ExperimentListItem } from "@/types/experiment";
import DesignDialog from "../../components/DesignDialog";
import { viewForState } from "../../viewForState";
import DuplicateDialog from "./DuplicateDialog";

/** A design can be edited until it is registered. */
export function isEditable(status: string): boolean {
  const view = viewForState(status);
  return view === "pricing" || view === "registrable" || view === "refused";
}

/**
 * The ⋯ menu in the experiment's header. A draft offers "Edit design", which
 * saves in place over the content hash it opened from (a hash someone else has
 * since replaced is the server's 409, shown in the dialog). A registered
 * experiment is final, so it offers a copy as a new draft instead. Nothing to
 * offer, no menu.
 */
export default function ExperimentMenu({ summary }: { summary: ExperimentListItem }) {
  const query = useExperimentDesign(summary.name, summary.status);
  const save = useSaveDesign(summary.name);
  const { is } = useRole();
  const [editing, setEditing] = useState(false);
  const [duplicating, setDuplicating] = useState(false);

  const design = query.data;
  if (!design || !is.workspaceEditor) return null;
  // A refused analysis of past data left no result: its design edits like a draft,
  // and saving reopens it (the refused run is discarded).
  const registered = design.frozen_at !== null && !design.reopenable;

  const submit = async (config: ExperimentDesignInput) => {
    await save.mutateAsync({ config, baseHash: design.content_hash });
    toast.success("Saved the draft design");
    setEditing(false);
  };

  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            variant='ghost'
            size='icon'
            className='h-8 w-8'
            aria-label='Experiment actions'
            data-testid='experiment-menu'
          >
            <MoreHorizontal className='h-4 w-4' />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align='end'>
          {registered ? (
            <DropdownMenuItem
              onClick={() => setDuplicating(true)}
              data-testid='experiment-duplicate-design'
            >
              <Copy className='h-4 w-4' />
              Duplicate as new draft
            </DropdownMenuItem>
          ) : (
            <DropdownMenuItem onClick={() => setEditing(true)} data-testid='experiment-edit-design'>
              <Pencil className='h-4 w-4' />
              Edit design
            </DropdownMenuItem>
          )}
        </DropdownMenuContent>
      </DropdownMenu>
      {!registered && (
        <DesignDialog
          open={editing}
          onOpenChange={setEditing}
          title='Edit design'
          description='Saved over the draft. If someone else saves first, this edit is refused and you can reload.'
          initial={design.config}
          submitLabel='Save design'
          onSubmit={submit}
        />
      )}
      {registered && (
        <DuplicateDialog open={duplicating} onOpenChange={setDuplicating} name={summary.name} />
      )}
    </>
  );
}
