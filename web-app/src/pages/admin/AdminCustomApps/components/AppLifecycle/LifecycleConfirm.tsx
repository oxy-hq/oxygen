import { type ReactNode, useState } from "react";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import {
  AlertDialog,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogTitle
} from "@/components/ui/shadcn/alert-dialog";
import { Button } from "@/components/ui/shadcn/button";
import type { ConfirmCopy } from "./lifecycleCopy";
import { OverrideStep } from "./OverrideStep";
import type { OverrideOffer } from "./planChecks";

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
 *
 * `override` is the one case with a second step: a promote held only by its checks,
 * which the caller may promote past. "Promote anyway…" then opens a step that takes
 * the reason and hands it to `onOverride`. Without an offer there is no such button.
 */
export const LifecycleConfirm = ({
  copy,
  pending,
  onConfirm,
  onCancel,
  notice,
  hold = false,
  testId = "app-lifecycle-confirm",
  override,
  onOverride,
  overrideError
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
  /** What a written reason would promote past, when the caller may. */
  override?: OverrideOffer;
  onOverride?: (reason: string) => void;
  /** Why the server refused the override, said under the reason. */
  overrideError?: string | null;
}) => (
  <AlertDialog open={copy !== null} onOpenChange={(open) => !open && !pending && onCancel()}>
    <AlertDialogContent
      // A little wider while the third button or its step shows (`data-wide`), so the
      // three fit one line and the dialog does not change width between the two.
      className={`${DESIGN_REFERENCE} gap-6 sm:max-w-md sm:has-[[data-wide]]:max-w-lg`}
      data-testid={testId}
    >
      <ConfirmSteps
        copy={copy}
        pending={pending}
        notice={notice}
        hold={hold}
        onConfirm={onConfirm}
        onCancel={onCancel}
        testId={testId}
        override={override}
        onOverride={onOverride}
        overrideError={overrideError}
      />
    </AlertDialogContent>
  </AlertDialog>
);

/** The dialog's body. Its own component so the open step is forgotten when the dialog
 *  closes: the content unmounts with it. */
const ConfirmSteps = ({
  copy,
  pending,
  notice,
  hold,
  onConfirm,
  onCancel,
  testId,
  override,
  onOverride,
  overrideError
}: {
  copy: ConfirmCopy | null;
  pending: boolean;
  notice?: ReactNode;
  hold: boolean;
  onConfirm: () => void;
  onCancel: () => void;
  testId: string;
  override?: OverrideOffer;
  onOverride?: (reason: string) => void;
  overrideError?: string | null;
}) => {
  // The offer the step was opened on: a refused act re-reads the plan, and the step
  // must not vanish under the reason while it does.
  const [past, setPast] = useState<OverrideOffer | null>(null);
  if (past && onOverride) {
    return (
      <OverrideStep
        offer={past}
        pending={pending}
        held={!override}
        error={overrideError ?? null}
        onBack={() => setPast(null)}
        onConfirm={onOverride}
        testId={`${testId}-override`}
      />
    );
  }
  const offered = override && onOverride ? override : null;
  const cancel = (
    <Button variant='ghost' disabled={pending} onClick={onCancel} data-testid={`${testId}-cancel`}>
      Cancel
    </Button>
  );
  const verb = (
    <Button disabled={pending || hold} onClick={onConfirm} data-testid={`${testId}-ok`}>
      {pending ? "Working…" : (copy?.confirm ?? "")}
    </Button>
  );
  return (
    <>
      <div className='flex flex-col gap-2'>
        <AlertDialogTitle>{copy?.title ?? ""}</AlertDialogTitle>
        <AlertDialogDescription>{copy?.description ?? ""}</AlertDialogDescription>
        {notice}
      </div>
      {offered ? (
        // The way past the checks sits apart from the two the confirm always has: on
        // the left, and on a line of its own where the three do not fit.
        <div className='flex flex-wrap justify-end gap-2' data-wide>
          <Button
            variant='outline'
            className='mr-auto'
            disabled={pending}
            onClick={() => setPast(offered)}
            data-testid={`${testId}-override`}
          >
            Promote anyway…
          </Button>
          <div className='flex gap-2'>
            {cancel}
            {verb}
          </div>
        </div>
      ) : (
        <div className='flex justify-end gap-2'>
          {cancel}
          {verb}
        </div>
      )}
    </>
  );
};
