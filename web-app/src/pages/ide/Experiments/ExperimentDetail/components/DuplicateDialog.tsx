import { useState } from "react";
import { useNavigate } from "react-router-dom";
import { toast } from "sonner";
import ErrorAlert from "@/components/ui/ErrorAlert";
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
import { useDuplicateExperiment } from "@/hooks/api/useExperiments";
import { useExperimentRoutes } from "../../useExperimentRoutes";

function DuplicateForm({ name, onClose }: { name: string; onClose: () => void }) {
  const duplicate = useDuplicateExperiment(name);
  const navigate = useNavigate();
  const routes = useExperimentRoutes();
  const [newName, setNewName] = useState(`${name}_copy`);
  const [error, setError] = useState<string | null>(null);

  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    const target = newName.trim();
    if (!target) return;
    setError(null);
    try {
      await duplicate.mutateAsync(target);
      toast.success(`Created ${target} as a new draft`);
      onClose();
      navigate(routes.DETAIL(target));
    } catch (e) {
      setError(e instanceof Error ? e.message : "The experiment could not be duplicated.");
    }
  };

  return (
    <DialogContent data-testid='experiment-duplicate-dialog'>
      <form onSubmit={submit} className='flex flex-col gap-4'>
        <DialogHeader>
          <DialogTitle>Duplicate {name}</DialogTitle>
          <DialogDescription>
            Creates a new draft with this design, under a new name.
          </DialogDescription>
        </DialogHeader>
        <Input
          value={newName}
          onChange={(e) => setNewName(e.target.value)}
          aria-label='New experiment name'
          data-testid='experiment-duplicate-name'
        />
        {error && (
          <div data-testid='experiment-duplicate-error'>
            <ErrorAlert title='The experiment was not created' message={error} />
          </div>
        )}
        <DialogFooter>
          <Button type='button' variant='ghost' onClick={onClose}>
            Cancel
          </Button>
          <Button
            type='submit'
            disabled={!newName.trim() || duplicate.isPending}
            data-testid='experiment-duplicate-submit'
          >
            Create draft
          </Button>
        </DialogFooter>
      </form>
    </DialogContent>
  );
}

/** Asks for the new experiment's name, copies the registered design into it and opens it. */
export default function DuplicateDialog({
  open,
  onOpenChange,
  name
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  name: string;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      {open && <DuplicateForm name={name} onClose={() => onOpenChange(false)} />}
    </Dialog>
  );
}
