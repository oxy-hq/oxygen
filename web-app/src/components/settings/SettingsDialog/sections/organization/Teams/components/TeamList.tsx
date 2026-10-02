import { Loader2, Pencil, Trash2, Users } from "lucide-react";
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
import { useDeleteTeam } from "@/hooks/api/appAccess";
import type { Team } from "@/types/appAccess";

export function TeamList({
  orgId,
  teams,
  isPending,
  isError,
  onEdit,
  onCreate
}: {
  orgId: string;
  teams: Team[];
  isPending: boolean;
  isError: boolean;
  onEdit: (team: Team) => void;
  onCreate: () => void;
}) {
  const deleteTeam = useDeleteTeam();
  const [pendingDelete, setPendingDelete] = useState<Team | null>(null);

  const handleDelete = async () => {
    if (!pendingDelete) return;
    try {
      await deleteTeam.mutateAsync({ orgId, teamId: pendingDelete.id });
      toast.success(`Deleted ${pendingDelete.name}`);
      setPendingDelete(null);
    } catch {
      toast.error("Couldn't delete the team");
    }
  };

  if (isPending) {
    return (
      <div className='flex items-center justify-center py-12'>
        <Loader2 className='size-5 animate-spin text-muted-foreground' aria-hidden />
        <span className='sr-only'>Loading teams</span>
      </div>
    );
  }

  if (isError) {
    return (
      <p className='py-12 text-center text-muted-foreground text-sm'>
        Couldn't load teams. Reopen this page to try again.
      </p>
    );
  }

  if (teams.length === 0) {
    return (
      <div className='flex flex-col items-center gap-2 rounded-lg border border-dashed px-6 py-12 text-center'>
        <Users className='size-6 text-muted-foreground' aria-hidden />
        <p className='font-medium text-sm'>No teams yet</p>
        <p className='max-w-sm text-muted-foreground text-xs leading-relaxed'>
          Create a team like “Finance” or “Store managers”, then grant it access to an app. New
          hires join the team once instead of being added to every app.
        </p>
        <Button size='sm' variant='outline' className='mt-2' onClick={onCreate}>
          Create the first team
        </Button>
      </div>
    );
  }

  return (
    <>
      <ul className='divide-y rounded-lg border'>
        {teams.map((team) => (
          <li key={team.id} className='flex items-center gap-3 px-4 py-3'>
            <Users className='size-4 shrink-0 text-muted-foreground' aria-hidden />
            <div className='min-w-0 flex-1'>
              <p className='truncate font-medium text-sm'>{team.name}</p>
              <p className='truncate text-muted-foreground text-xs'>
                {team.member_count} {team.member_count === 1 ? "person" : "people"}
                {team.description ? ` · ${team.description}` : ""}
              </p>
            </div>
            <Button
              variant='ghost'
              size='sm'
              onClick={() => onEdit(team)}
              aria-label={`Edit ${team.name}`}
            >
              <Pencil className='size-4' aria-hidden />
              Edit
            </Button>
            <Button
              variant='ghost'
              size='icon'
              className='size-8 text-muted-foreground hover:text-destructive'
              onClick={() => setPendingDelete(team)}
              aria-label={`Delete ${team.name}`}
            >
              <Trash2 className='size-4' aria-hidden />
            </Button>
          </li>
        ))}
      </ul>

      <AlertDialog
        open={pendingDelete !== null}
        onOpenChange={(open) => !open && setPendingDelete(null)}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Delete {pendingDelete?.name}?</AlertDialogTitle>
            <AlertDialogDescription>
              Anyone who could only open an app through this team loses access right away. The
              people themselves stay in the organization.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Keep team</AlertDialogCancel>
            <AlertDialogAction
              onClick={(e) => {
                e.preventDefault();
                // handleDelete toasts its own failure and cannot reject.
                void handleDelete();
              }}
              disabled={deleteTeam.isPending}
            >
              {deleteTeam.isPending && <Loader2 className='size-4 animate-spin' aria-hidden />}
              Delete team
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}
