import { useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/shadcn/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle
} from "@/components/ui/shadcn/dialog";
import { Input } from "@/components/ui/shadcn/input";
import { Label } from "@/components/ui/shadcn/label";
import { useRenameRole } from "@/hooks/api/organizations";
import { apiErrorMessage, apiStatus } from "@/libs/apiError";
import type { RoleRow } from "@/types/operatingGraph";

export function RenamePositionDialog({
  open,
  onOpenChange,
  orgId,
  role
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  orgId: string;
  role: RoleRow;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className='sm:max-w-sm'>
        <DialogHeader>
          <DialogTitle>Rename {role.name}</DialogTitle>
          <DialogDescription>Everyone who holds it keeps it under the new name.</DialogDescription>
        </DialogHeader>
        <RenamePositionForm orgId={orgId} role={role} onDone={() => onOpenChange(false)} />
      </DialogContent>
    </Dialog>
  );
}

function RenamePositionForm({
  orgId,
  role,
  onDone
}: {
  orgId: string;
  role: RoleRow;
  onDone: () => void;
}) {
  const rename = useRenameRole();
  const [name, setName] = useState(role.name);
  const [error, setError] = useState<string | null>(null);
  const unchanged = name.trim() === role.name;
  const canSubmit = name.trim().length > 0 && !unchanged;

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!canSubmit || rename.isPending) return;
    setError(null);
    try {
      await rename.mutateAsync({ orgId, roleId: role.id, name: name.trim() });
      toast.success(`Renamed to ${name.trim()}`);
      onDone();
    } catch (err) {
      if (apiStatus(err) === 409) {
        setError("A position with that name already exists. Pick another name.");
        return;
      }
      setError(apiErrorMessage(err, "Couldn't rename the position"));
    }
  };

  return (
    <form onSubmit={handleSubmit} className='flex flex-col gap-4 pt-1'>
      <div className='space-y-1.5'>
        <Label htmlFor='position-rename'>Name</Label>
        <Input
          id='position-rename'
          value={name}
          onChange={(e) => setName(e.target.value)}
          required
          autoFocus
          data-testid='settings-positions-rename-name'
        />
      </div>
      {error && <p className='text-destructive text-sm'>{error}</p>}
      <div className='flex justify-end gap-2'>
        <Button type='button' variant='outline' size='sm' onClick={onDone}>
          Cancel
        </Button>
        <Button
          type='submit'
          size='sm'
          disabled={!canSubmit || rename.isPending}
          data-testid='settings-positions-rename-submit'
        >
          {rename.isPending ? "Saving…" : "Save changes"}
        </Button>
      </div>
    </form>
  );
}
