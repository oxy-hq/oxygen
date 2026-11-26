import type { ReactNode } from "react";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import {
  AlertDialog,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogTitle
} from "@/components/ui/shadcn/alert-dialog";
import { Button } from "@/components/ui/shadcn/button";
import type { ConfirmCopy } from "./lifecycleCopy";

/**
 * The one confirmation every lifecycle action goes through. One plain step: a title,
 * a sentence saying what customers see next, Cancel, and the verb.
 *
 * `copy` doubles as the open flag — `null` closes it — so a caller holds exactly one
 * piece of state ("what am I about to do") rather than an open boolean beside it.
 *
 * `notice` is said under the sentence — the admin console's check of the build about
 * to go live — and `hold` keeps the verb disabled while that check is still out, so it
 * cannot be clicked past before it renders.
 */
export const LifecycleConfirm = ({
  copy,
  pending,
  onConfirm,
  onCancel,
  notice,
  hold = false,
  testId = "app-lifecycle-confirm"
}: {
  copy: ConfirmCopy | null;
  pending: boolean;
  notice?: ReactNode;
  hold?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
  /** The dialog's testid; `-ok` and `-cancel` hang off it. Another panel borrowing
   *  this one-step confirm (a sandbox teardown) names itself here. */
  testId?: string;
}) => (
  <AlertDialog open={copy !== null} onOpenChange={(open) => !open && !pending && onCancel()}>
    <AlertDialogContent className={`${DESIGN_REFERENCE} gap-6 sm:max-w-md`} data-testid={testId}>
      <div className='flex flex-col gap-2'>
        <AlertDialogTitle>{copy?.title ?? ""}</AlertDialogTitle>
        <AlertDialogDescription>{copy?.description ?? ""}</AlertDialogDescription>
        {notice}
      </div>
      <div className='flex justify-end gap-2'>
        <Button
          variant='ghost'
          disabled={pending}
          onClick={onCancel}
          data-testid={`${testId}-cancel`}
        >
          Cancel
        </Button>
        <Button disabled={pending || hold} onClick={onConfirm} data-testid={`${testId}-ok`}>
          {pending ? "Working…" : (copy?.confirm ?? "")}
        </Button>
      </div>
    </AlertDialogContent>
  </AlertDialog>
);
