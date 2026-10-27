import { useState } from "react";
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
import { useExperimentDesign, useRegisterExperiment } from "@/hooks/api/useExperiments";
import { useRole } from "@/hooks/useRole";
import { StatGrid, StatTile } from "@/pages/ide/Experiments/components/glance/StatTile";
import { ToneBadge } from "@/pages/ide/Experiments/components/glance/tone";
import { formatInstant } from "@/pages/ide/Experiments/dates";
import { COLUMN } from "@/pages/ide/Experiments/ExperimentDetail/column";
import { NOT_AVAILABLE } from "@/pages/ide/Experiments/format";
import type { ExperimentListItem } from "@/types/experiment";

/** Whole days from today to an ISO date, as words; null when there is no date. */
function timeLeft(iso: string | null): string | undefined {
  if (!iso) return undefined;
  const target = new Date(`${iso}T00:00:00`);
  if (Number.isNaN(target.getTime())) return undefined;
  const days = Math.ceil((target.getTime() - Date.now()) / 86_400_000);
  if (days < 0) return "date has passed";
  if (days === 0) return "today";
  return `${days} ${days === 1 ? "day" : "days"} left`;
}

/**
 * The design could be registered right now, and stays registrable until someone
 * registers it (or the deadline passes). Registering is explicit, irreversible
 * and Owner/Admin only; the confirm dialog says registering is final. The request
 * carries the content hash that was read, so a design edited since is refused.
 */
export default function RegistrableView({ summary }: { summary: ExperimentListItem }) {
  const designQuery = useExperimentDesign(summary.name);
  const register = useRegisterExperiment(summary.name);
  const { is } = useRole();
  const [open, setOpen] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const design = designQuery.data;

  const close = (next: boolean) => {
    setOpen(next);
    if (!next) setError(null);
  };

  const confirm = async () => {
    if (!design) return;
    setError(null);
    try {
      await register.mutateAsync(design.content_hash);
      toast.success(`Registered ${summary.name}`);
      setOpen(false);
    } catch (e) {
      setError(e instanceof Error ? e.message : "The registration failed.");
    }
  };

  return (
    <div className={COLUMN} data-testid='experiment-registrable'>
      <section className='flex flex-col gap-4 rounded-lg border bg-card p-4'>
        <div className='flex flex-wrap items-center justify-between gap-2'>
          <h2 className='font-semibold text-base'>Register this design</h2>
          <ToneBadge tone='info' testId='experiment-registrable-status'>
            Ready to register
          </ToneBadge>
        </div>
        <StatGrid className='sm:grid-cols-2'>
          <StatTile
            id='registrable-design'
            testId='experiment-registrable-design'
            label='Design'
            value='Draft'
            sub={design ? `last edited ${formatInstant(design.updated_at)}` : undefined}
          />
          <StatTile
            id='registrable-deadline'
            testId='experiment-registrable-deadline'
            label='Register before'
            value={summary.next_switch_on ?? NOT_AVAILABLE}
            sub={timeLeft(summary.next_switch_on)}
            tone={summary.next_switch_on ? "warning" : undefined}
            hint='Nothing registers automatically: the design stays registrable until someone registers it or the first switch date passes.'
          />
        </StatGrid>
        <div className='flex flex-wrap items-center gap-3'>
          <Button
            disabled={!design || !is.workspaceAdmin}
            title={is.workspaceAdmin ? undefined : "Only workspace owners and admins can register."}
            onClick={() => setOpen(true)}
            data-testid='experiment-register'
          >
            Register
          </Button>
          {!is.workspaceAdmin && (
            <span className='text-muted-foreground text-xs'>
              Only workspace owners and admins can register.
            </span>
          )}
        </div>
      </section>
      <Dialog open={open} onOpenChange={close}>
        <DialogContent data-testid='experiment-register-dialog'>
          <DialogHeader>
            <DialogTitle>Register {summary.name}?</DialogTitle>
            <DialogDescription>
              Registering is final: the design can no longer be edited, and it cannot be undone. The
              platform starts flipping the lever on schedule.
            </DialogDescription>
          </DialogHeader>
          {error && (
            <div data-testid='experiment-register-error'>
              <ErrorAlert title='The experiment was not registered' message={error} />
            </div>
          )}
          <DialogFooter>
            <Button variant='ghost' onClick={() => close(false)}>
              Not yet
            </Button>
            <Button
              disabled={!design || register.isPending}
              onClick={confirm}
              data-testid='experiment-register-confirm'
            >
              Register
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
