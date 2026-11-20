// Started from pokehouse-oxy `apps/store-ops/src/views/Tablets.tsx` `RowActions` and `DeviceRow`
// (INVENTORY section 5; decision 43, Option A: main 0.6.4's words and place, drawn in the design
// system). Kept: the question asked in the row's own action cell in place of the buttons that
// asked it, Keep it taking focus as the question appears, the confirming action the primary,
// focus back to the control that asked after Keep it, the row's neutral wash while it asks. New:
// it is a piece, not a row's own state; while busy both answers keep focus (the Button's `busy`)
// instead of `disabled`; Escape keeps it, as Keep it does; and the question names the group the
// two answers sit in.
//
// **Why a second piece beside ConfirmDialog, not one with a mode.** The two share a rule (the safe
// answer first, the confirm the primary, busy keeps focus) and nothing of their frame. The dialog
// owns the top layer, traps focus and scrims the page; this one sits in a table cell, traps
// nothing, and the rest of the page stays live. One component would carry a title, a history
// entry and a backdrop that mean nothing on a row, and a row wash that means nothing in a dialog.
//
// The row is the caller's: give it `confirmRowClass` while `open`, the neutral wash (a question
// waiting on an answer is a process state, not a verdict, so it is not the error tone).

import { type ReactNode, useId, useLayoutEffect, useRef } from "react";
import { Button, type ButtonSize } from "../actions/Button";
import { cx } from "../cx";
import { focusableIn, openerFor } from "../focus/modalFocus";

/** The row's wash while its question is open. */
export const confirmRowClass = "oxy-ui-confirm-row";

export interface ConfirmInlineProps {
  /** The question shows in place of `children`. */
  open: boolean;
  /** What happens, in a sentence: "Revoke Front counter iPad? Crew can't sign in on it after
   *  this, and it can't be undone." */
  question: ReactNode;
  /** The confirming answer: "Revoke it". */
  confirmLabel: string;
  /** The safe answer. Default "Keep it". */
  cancelLabel?: string;
  /** The action is in flight: both answers keep focus and ignore presses. */
  busy?: boolean;
  /** The confirm's word while busy: "Revoking…". */
  busyLabel?: string;
  /** `sm` on a desk row, `lg` on a phone card. Default `sm`. */
  size?: ButtonSize;
  /** `end` in a table's actions column (the default); `start` on a card. */
  align?: "start" | "end";
  onConfirm: () => void;
  onCancel: () => void;
  /** The controls that ask (New link, Revoke): hidden while the question shows, and where focus
   *  goes back after Keep it. */
  children: ReactNode;
}

export function ConfirmInline({
  open,
  question,
  confirmLabel,
  cancelLabel = "Keep it",
  busy = false,
  busyLabel,
  size = "sm",
  align = "end",
  onConfirm,
  onCancel,
  children
}: ConfirmInlineProps) {
  const questionId = useId();
  const triggers = useRef<HTMLSpanElement>(null);
  const keep = useRef<HTMLButtonElement>(null);
  const opener = useRef<HTMLElement | null>(null);
  const was = useRef(open);

  useLayoutEffect(() => {
    if (open === was.current) return;
    was.current = open;
    const box = triggers.current;
    if (open) {
      // The control that asked: focused, or (Safari focuses nothing on a press) the one pressed.
      const asked = openerFor(document.activeElement);
      opener.current = asked && box?.contains(asked) ? asked : null;
      keep.current?.focus();
      return;
    }
    const back = opener.current?.isConnected ? opener.current : box ? focusableIn(box)[0] : null;
    opener.current = null;
    const active = document.activeElement;
    // The answer that had focus has gone with the question; a caller that moved focus on purpose
    // (a refusal line) keeps it there.
    if (!active || active === document.body) back?.focus();
  }, [open]);

  return (
    <span className={cx("oxy-ui-confirm-inline", align === "end" && "oxy-ui-confirm-inline--end")}>
      <span ref={triggers} className='oxy-ui-confirm-inline__triggers' hidden={open}>
        {children}
      </span>
      {open && (
        // biome-ignore lint/a11y/useSemanticElements: a group of two answers, not a fieldset of fields.
        <span
          role='group'
          aria-labelledby={questionId}
          className='oxy-ui-confirm-inline__ask'
          onKeyDown={(e) => {
            if (e.key !== "Escape" || e.defaultPrevented) return;
            e.preventDefault();
            if (!busy) onCancel();
          }}
        >
          <span id={questionId} className='oxy-ui-confirm-inline__question'>
            {question}
          </span>
          <Button
            ref={keep}
            size={size}
            aria-disabled={busy || undefined}
            onClick={() => {
              if (!busy) onCancel();
            }}
          >
            {cancelLabel}
          </Button>
          <Button
            variant='primary'
            size={size}
            busy={busy}
            busyLabel={busyLabel}
            onClick={() => {
              if (!busy) onConfirm();
            }}
          >
            {confirmLabel}
          </Button>
        </span>
      )}
    </span>
  );
}
