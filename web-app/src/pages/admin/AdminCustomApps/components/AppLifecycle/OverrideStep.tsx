import { Check } from "lucide-react";
import { useId, useState } from "react";
import { AlertDialogDescription, AlertDialogTitle } from "@/components/ui/shadcn/alert-dialog";
import { Button } from "@/components/ui/shadcn/button";
import { Textarea } from "@/components/ui/shadcn/textarea";
import { cn } from "@/libs/shadcn/utils";
import { AdminStatusPill } from "@/pages/admin/components/AdminStatusPill";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import { overrideTitle } from "../AppConsole/productionGates";
import { type OverrideOffer, reasonState } from "./planChecks";

/**
 * Promote past a check: what is being promoted past, as chips, and the reason, which
 * is the whole step. The button waits for the reason the server will accept (ten
 * characters after trimming) and the counter says how far it is. A refusal of the
 * override itself (`override_not_allowed`) is said under the reason. `held` rests the
 * button while there is no plan to act on: after a refused act the plan is re-read,
 * and it may no longer offer the override at all.
 */
export const OverrideStep = ({
  offer,
  pending,
  held,
  error,
  onBack,
  onConfirm,
  testId
}: {
  offer: OverrideOffer;
  pending: boolean;
  held: boolean;
  error: string | null;
  onBack: () => void;
  onConfirm: (reason: string) => void;
  testId: string;
}) => {
  const [reason, setReason] = useState("");
  const id = useId();
  const state = reasonState(reason);
  return (
    <>
      <div className='flex flex-col gap-3' data-testid={`${testId}-step`} data-wide>
        <AlertDialogTitle>{overrideTitle(offer.codes)}</AlertDialogTitle>
        <AlertDialogDescription asChild>
          <div className='flex flex-wrap gap-1.5' data-testid={`${testId}-past`}>
            {offer.past.map((p) => (
              <AdminStatusPill key={p.label} tone={p.tone} label={p.label} />
            ))}
          </div>
        </AlertDialogDescription>
        <div className='flex flex-col gap-1.5'>
          <div className='flex items-baseline justify-between gap-3'>
            <label htmlFor={id} className='font-medium text-xs'>
              Reason, kept in the audit log
            </label>
            <span
              className={cn(
                "inline-flex shrink-0 items-center gap-1 text-xs tabular-nums",
                state.ok ? ADMIN_TONE.ok.text : "text-muted-foreground"
              )}
              data-testid={`${testId}-count`}
              data-ok={state.ok}
            >
              {state.ok && <Check className='size-3' aria-hidden />}
              {state.text}
            </span>
          </div>
          <Textarea
            id={id}
            value={reason}
            onChange={(e) => setReason(e.target.value)}
            disabled={pending}
            autoFocus
            aria-invalid={error ? true : undefined}
            data-testid={`${testId}-reason`}
          />
          {error && (
            <p
              role='alert'
              className={cn("text-xs", ADMIN_TONE.danger.text)}
              data-testid={`${testId}-refused`}
            >
              {error}
            </p>
          )}
        </div>
      </div>
      <div className='flex flex-wrap justify-end gap-2'>
        <Button variant='ghost' disabled={pending} onClick={onBack} data-testid={`${testId}-back`}>
          Back
        </Button>
        <Button
          disabled={pending || held || !state.ok}
          onClick={() => onConfirm(reason)}
          data-testid={`${testId}-ok`}
        >
          {pending ? "Working…" : "Promote anyway"}
        </Button>
      </div>
    </>
  );
};
