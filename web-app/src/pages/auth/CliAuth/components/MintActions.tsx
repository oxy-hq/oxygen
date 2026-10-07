import type React from "react";
import { KeyHint } from "@/components/ui/KeyHint";
import { Button } from "@/components/ui/shadcn/button";
import { Spinner } from "@/components/ui/shadcn/spinner";

interface Props {
  /**
   * Why the request can't be approved as it stands, and what to do about it. Present exactly
   * when Approve can never turn on for this request.
   */
  problem?: React.ReactNode;
  /** What the server said when it refused the approval. */
  refusal: string | null;
  /** The approval is in flight: both buttons are off until it answers. */
  pending: boolean;
  /** There is a request to approve, and it has been in front of the person for a moment. */
  canApprove: boolean;
  onApprove: () => void;
  onCancel: () => void;
  /** The button names what it approves when a look alone could mistake it for something else. */
  approveLabel?: string;
}

/**
 * The foot of every token request: what is wrong with it, if anything, then Cancel and Approve.
 *
 * Approving is a click on Approve, or the button focused and pressed. No shortcut does it, and
 * neither button takes focus on load. Escape cancels, and Cancel shows the key.
 */
const MintActions: React.FC<Props> = ({
  problem,
  refusal,
  pending,
  canApprove,
  onApprove,
  onCancel,
  approveLabel = "Approve"
}) => (
  <>
    {problem && (
      <p
        className='mt-2 font-medium text-sm leading-5.5'
        role='alert'
        data-testid='cli-auth-mint-problem'
      >
        {problem}
      </p>
    )}
    {refusal && (
      <p
        className='mt-4 text-destructive text-sm leading-5.5'
        role='alert'
        data-testid='cli-auth-error'
      >
        {refusal}
      </p>
    )}

    <div className='mt-6 flex justify-end gap-2'>
      <Button
        variant='outline'
        className='px-3'
        onClick={onCancel}
        disabled={pending}
        aria-keyshortcuts='Escape'
        data-testid='cli-auth-cancel'
      >
        Cancel
        <KeyHint className='-mr-1'>Esc</KeyHint>
      </Button>
      <Button onClick={onApprove} disabled={!canApprove} data-testid='cli-auth-confirm'>
        {pending && <Spinner className='size-4' />}
        {approveLabel}
      </Button>
    </div>
  </>
);

export default MintActions;
