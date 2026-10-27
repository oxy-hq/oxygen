import { useState } from "react";
import ErrorAlert from "@/components/ui/ErrorAlert";
import { Alert, AlertDescription } from "@/components/ui/shadcn/alert";
import { Button } from "@/components/ui/shadcn/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle
} from "@/components/ui/shadcn/dialog";
import { Label } from "@/components/ui/shadcn/label";
import { Textarea } from "@/components/ui/shadcn/textarea";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/shadcn/toggle-group";
import { useAbortExperiment } from "@/hooks/api/useExperiments";
import { ToneIcon } from "@/pages/ide/Experiments/components/glance/tone";
import type { AbortLevers } from "@/types/experiment";
import AbortEffects, { unitList } from "./components/AbortEffects";

/**
 * Abort is explicit and recorded: it ends the run and notifies the owner, so it
 * needs a reason. By default it runs the off lever at every unit that is on; the
 * owner may instead keep the levers as they are, and the run stops managing them.
 * The server records it (202) and the next tick actuates. The error stays in the
 * dialog — the person cannot proceed until they read it.
 */
export default function AbortDialog({
  open,
  onOpenChange,
  name,
  owner,
  unitsOn
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  name: string;
  owner: string;
  unitsOn: string[];
}) {
  const abort = useAbortExperiment(name);
  const [reason, setReason] = useState("");
  const [levers, setLevers] = useState<AbortLevers>("off");
  const [error, setError] = useState<string | null>(null);
  const trimmed = reason.trim();

  const close = (next: boolean) => {
    onOpenChange(next);
    if (!next) {
      setReason("");
      setLevers("off");
      setError(null);
    }
  };

  const confirm = async () => {
    setError(null);
    try {
      await abort.mutateAsync({ reason: trimmed, levers });
      close(false);
    } catch (e) {
      setError(e instanceof Error ? e.message : "The abort request failed.");
    }
  };

  return (
    <Dialog open={open} onOpenChange={close}>
      <DialogContent data-testid='experiment-abort-dialog'>
        <DialogHeader>
          <DialogTitle>Abort {name}?</DialogTitle>
          <DialogDescription>The run stops now. Aborting:</DialogDescription>
        </DialogHeader>
        <div className='flex flex-col gap-1.5'>
          <Label>Levers</Label>
          <ToggleGroup
            type='single'
            variant='outline'
            value={levers}
            onValueChange={(v) => v && setLevers(v as AbortLevers)}
            className='w-full'
          >
            <ToggleGroupItem
              value='off'
              className='flex-1'
              data-testid='experiment-abort-levers-off'
            >
              Turn levers off
            </ToggleGroupItem>
            <ToggleGroupItem
              value='keep'
              className='flex-1'
              data-testid='experiment-abort-levers-keep'
            >
              Keep levers as they are
            </ToggleGroupItem>
          </ToggleGroup>
        </div>
        <AbortEffects levers={levers} unitsOn={unitsOn} owner={owner} />
        {levers === "keep" && (
          <Alert data-testid='experiment-abort-keep-warning'>
            <ToneIcon tone='warning' className='size-4' />
            <AlertDescription>
              {unitsOn.length > 0
                ? `The experiment will no longer manage ${unitList(unitsOn)}. Turn their levers off yourself when you are done.`
                : "The experiment will no longer manage any unit's lever."}
            </AlertDescription>
          </Alert>
        )}
        <div className='flex flex-col gap-1.5'>
          <Label htmlFor='experiment-abort-reason'>Reason</Label>
          <Textarea
            id='experiment-abort-reason'
            data-testid='experiment-abort-reason'
            value={reason}
            onChange={(e) => setReason(e.target.value)}
            placeholder='Why is this run being stopped?'
          />
          <p className='text-muted-foreground text-xs'>Recorded with the run and on its report.</p>
        </div>
        {error && (
          <div data-testid='experiment-abort-error'>
            <ErrorAlert title='The experiment was not aborted' message={error} />
          </div>
        )}
        <DialogFooter>
          <Button variant='ghost' onClick={() => close(false)}>
            Keep running
          </Button>
          <Button
            variant='destructive'
            disabled={trimmed === "" || abort.isPending}
            onClick={confirm}
            data-testid='experiment-abort-confirm'
          >
            Abort experiment
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
