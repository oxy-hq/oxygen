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
import { useSetWorkerApps } from "@/hooks/api/organizations";
import type { AppAccessSummary } from "@/types/appAccess";
import type { FrontlineWorker } from "@/types/frontline";
import { apiErrorMessage, apiStatus, sameIds } from "../utils";
import { AppChecklist } from "./AppChecklist";

export function WorkerAppsDialog({
  open,
  onOpenChange,
  orgId,
  worker,
  apps
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  orgId: string;
  worker: FrontlineWorker;
  apps: AppAccessSummary[];
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className='sm:max-w-sm'>
        <DialogHeader>
          <DialogTitle>Apps for {worker.name}</DialogTitle>
          <DialogDescription>
            What they can open after signing in. Saving replaces the whole list.
          </DialogDescription>
        </DialogHeader>
        {/* Mounted fresh on every open, so the checklist always seeds from the
            row as it is now — never from an edit abandoned last time. */}
        <WorkerAppsForm
          orgId={orgId}
          worker={worker}
          apps={apps}
          onDone={() => onOpenChange(false)}
        />
      </DialogContent>
    </Dialog>
  );
}

function WorkerAppsForm({
  orgId,
  worker,
  apps,
  onDone
}: {
  orgId: string;
  worker: FrontlineWorker;
  apps: AppAccessSummary[];
  onDone: () => void;
}) {
  const setApps = useSetWorkerApps();
  const [selected, setSelected] = useState<string[]>(worker.apps);
  const [error, setError] = useState<string | null>(null);
  const unchanged = sameIds(selected, worker.apps);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (unchanged) return;
    setError(null);
    try {
      await setApps.mutateAsync({ orgId, userId: worker.user_id, apps: selected });
      toast.success(`Updated apps for ${worker.name}`);
      onDone();
    } catch (err) {
      if (apiStatus(err) === 403) {
        setError("You can't grant apps in this organization.");
        return;
      }
      setError(apiErrorMessage(err, "Couldn't update the apps"));
    }
  };

  return (
    <form onSubmit={handleSubmit} className='flex flex-col gap-4 pt-1'>
      <AppChecklist idPrefix='worker-app' apps={apps} selected={selected} onChange={setSelected} />
      {error && <p className='text-destructive text-sm'>{error}</p>}
      <div className='flex justify-end gap-2'>
        <Button type='button' variant='outline' size='sm' onClick={onDone}>
          Cancel
        </Button>
        <Button
          type='submit'
          size='sm'
          disabled={unchanged || setApps.isPending}
          data-testid='settings-crew-apps-submit'
        >
          {setApps.isPending ? "Saving…" : "Save apps"}
        </Button>
      </div>
    </form>
  );
}
