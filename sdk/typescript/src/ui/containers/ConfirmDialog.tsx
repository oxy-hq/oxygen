// Started from pokehouse-oxy `apps/warehouse/src/components/ConfirmDialog.tsx` (INVENTORY section 5:
// desk and phone in one, tested by warehouse's `reportDetailPhone.test.ts` and `tapTargets.test.ts`;
// Bookkeeping's is desk only). Kept: the name and props, the native-<dialog> `Modal` sized to its
// words with the answers pinned under the body, Cancel focused first, the confirm the primary, a
// held history entry on the phone. Changed: while an answer is in flight both answers keep focus
// and refuse a second press (the Button's `busy`: `aria-busy` + `aria-disabled`) instead of
// `disabled`, which dropped focus to <body>; and the phone form is the Modal's own (Modal.tsx).
//
// A confirm step in the centre of the window: Delete a report, discard unsaved changes. A real
// decision point rather than a pill that grew out of the button that asked. For a question that
// belongs on the row that asked it (Revoke a tablet, decision 43), use `ConfirmInline`.
//
// **The safe answer takes focus first** (decision 43), so a reflexive Enter cancels rather than
// confirms: Cancel on the desk, the sheet's Back (labelled with the cancel word) on a phone.
//
// **On a phone** it is the kit's full-screen sheet with Back: the answers are the kit's 48px
// buttons in its footer, the confirm the primary as on the desk. The kit's danger tone is not
// used: the design has three button variants and no red one.

import { type ReactNode, useRef } from "react";
import { PhoneButton } from "../../shell/phone/PhoneButton";
import { usePhoneLayout } from "../../shell/phone/usePhoneLayout";
import { Button } from "../actions/Button";
import { Modal } from "./Modal";

export interface ConfirmDialogProps {
  title: string;
  /** What is being changed or removed, and the consequence. */
  children: ReactNode;
  /** The confirming answer's word. Default "Delete". */
  confirmLabel?: string;
  /** The safe answer's word. Default "Cancel". */
  cancelLabel?: string;
  /** The action is in flight: both answers keep focus and ignore presses, and Escape, the
   *  backdrop and Back do nothing. */
  busy?: boolean;
  /** The confirm's word while busy. Default "Working…". */
  busyLabel?: string;
  /** Phone only: hold a history entry while the sheet is open, so the phone's Back cancels. Off
   *  for a question itself asked from a held Back (an unsaved-changes guard), where an entry of
   *  its own would shift the step the router walks back. */
  historyEntry?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}

export function ConfirmDialog({
  title,
  children,
  confirmLabel = "Delete",
  cancelLabel = "Cancel",
  busy = false,
  busyLabel = "Working…",
  historyEntry = true,
  onConfirm,
  onCancel
}: ConfirmDialogProps) {
  const phone = usePhoneLayout();
  const cancelRef = useRef<HTMLButtonElement>(null);
  const cancel = () => {
    if (!busy) onCancel();
  };
  const confirm = () => {
    if (!busy) onConfirm();
  };

  const footer = phone ? (
    <>
      <PhoneButton tone='secondary' onClick={cancel}>
        {cancelLabel}
      </PhoneButton>
      <PhoneButton tone='primary' onClick={confirm}>
        {busy ? busyLabel : confirmLabel}
      </PhoneButton>
    </>
  ) : (
    <>
      <Button ref={cancelRef} onClick={cancel} aria-disabled={busy || undefined}>
        {cancelLabel}
      </Button>
      <Button variant='primary' onClick={confirm} busy={busy} busyLabel={busyLabel}>
        {confirmLabel}
      </Button>
    </>
  );

  return (
    <Modal
      open
      compact
      busy={busy}
      title={title}
      onClose={cancel}
      initialFocus={phone ? undefined : cancelRef}
      backLabel={cancelLabel}
      historyEntry={historyEntry}
      footer={footer}
    >
      <div className='oxy-ui-confirm__body'>{children}</div>
    </Modal>
  );
}
