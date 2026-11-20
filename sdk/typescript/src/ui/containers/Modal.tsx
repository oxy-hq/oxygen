// Started from oxy-hq/reference-app `src/components/Modal.tsx` (INVENTORY section 2: the structure;
// its three copies add to it and none has tests), with pokehouse-oxy warehouse's
// `components/Modal.tsx` (`compact`, `footer`, `busy`, a unique title id, focus back on an
// unmount while open) and watchlist's scrim (the theme's `--scrim`, blurred by the theme,
// decision 35). Kept from the reference: the native <dialog>, `ModalFrame`, `initialFocus`, the
// two-beat close (the exit fade runs on a still-mounted element, then the parent is told), the
// reconcile-every-render effect, Escape on keydown, the press on the backdrop, no pointer while
// fading out, the page behind pinned. New: Tab wraps inside and focus goes back to the opener
// (focus/modalFocus.ts, shared with SidePanel); reduced motion closes at once instead of waiting
// for a fade that never runs; an Escape a layer inside took is left to it; and on a phone the
// same props open the kit's full-screen sheet (the reference's `phone/ModalSheet.tsx`), unless
// the app provides its own `ModalFrame`.
//
// A dialog in the centre of the window (decision 41). Native rather than hand-rolled because
// `showModal()` gives what hand-rolled modals get wrong: the background goes inert, Escape is
// there, and the dialog renders in the top layer, above the shell's rail and dock whatever their
// z-index. What it does not give, and is added here: closing on a backdrop press, Tab that wraps
// instead of leaving for the browser's toolbar, focus back to the opener when the parent unmounts
// it, and the page behind held still.
//
// **The element is told, never asked.** Nothing listens for the dialog's own `close` / `cancel`
// events: measured in the reference, neither fires reliably, and a design that learns about
// closing FROM the element is one quirk away from a dialog that looks like it ignores Escape while
// the page stays locked behind it. Every close path (Escape, the backdrop, the X) calls
// `requestClose`, and an effect drives the element to match React's state on every render.

import {
  createContext,
  type ReactNode,
  type RefObject,
  useCallback,
  useContext,
  useEffect,
  useId,
  useRef,
  useState
} from "react";
import { PhoneSheet } from "../../shell/phone/PhoneSheet";
import { usePhoneLayout } from "../../shell/phone/usePhoneLayout";
import { Button } from "../actions/Button";
import { cx } from "../cx";
import { useModalFocus } from "../focus/modalFocus";
import { XIcon } from "../icons";

/** The exit fade's backstop: the slow motion step plus a frame or two. A transition that never
 *  starts never ends (a backgrounded tab, no `allow-discrete`), and without this the dialog would
 *  sit invisible and mounted while the app believed it open. */
const EXIT_FALLBACK_MS = 450;

export interface ModalProps {
  open: boolean;
  onClose: () => void;
  /** The dialog's accessible name, drawn as its title. */
  title: ReactNode;
  /** Where focus lands on open, when the panel is the wrong answer: a search-first picker's box,
   *  a question's safe answer. Default: the body, so nothing looks pre-selected. */
  initialFocus?: RefObject<HTMLElement | null>;
  /** Sized to its words, for a question rather than a record to read. */
  compact?: boolean;
  /** Pinned under the body: a question's answers. */
  footer?: ReactNode;
  /** An answer is in flight: Escape, the backdrop and the X do nothing. */
  busy?: boolean;
  /** Phone: the sheet's Back label. Default "Back". */
  backLabel?: string;
  /** Phone: the sheet holds a history entry so the phone's Back closes it (default true). */
  historyEntry?: boolean;
  children: ReactNode;
}

/**
 * What draws a `Modal` instead, when something above it says so: a layout that wants its own
 * frame (a phone app that draws every dialog as its own sheet). Without one, the desk draws the
 * dialog below and a phone the kit's `PhoneSheet`.
 */
export const ModalFrame = createContext<((props: ModalProps) => ReactNode) | null>(null);

export function Modal(props: ModalProps) {
  const frame = useContext(ModalFrame);
  const phone = usePhoneLayout();
  if (frame) return frame(props);
  return phone ? <ModalSheet {...props} /> : <DialogModal {...props} />;
}

/** The phone's form: a full-screen sheet with Back, which the phone's Back also closes. A centred
 *  panel with a close X over a 390px screen is a desk idiom. */
function ModalSheet({
  open,
  onClose,
  title,
  initialFocus,
  footer,
  busy = false,
  backLabel,
  historyEntry,
  children
}: ModalProps) {
  // The sheet focuses its Back as it opens; a caller's own first focus comes after it.
  useEffect(() => {
    if (!open || !initialFocus) return;
    const t = window.setTimeout(() => initialFocus.current?.focus(), 0);
    return () => window.clearTimeout(t);
  }, [open, initialFocus]);
  return (
    <PhoneSheet
      open={open}
      onClose={() => {
        if (!busy) onClose();
      }}
      title={title}
      backLabel={backLabel}
      historyEntry={historyEntry}
      footer={footer}
    >
      {children}
    </PhoneSheet>
  );
}

function prefersReducedMotion(): boolean {
  return (
    typeof window !== "undefined" &&
    typeof window.matchMedia === "function" &&
    window.matchMedia("(prefers-reduced-motion: reduce)").matches
  );
}

const isOpen = (el: HTMLDialogElement) => el.open || el.hasAttribute("open");

function DialogModal({
  open,
  onClose,
  title,
  initialFocus,
  compact = false,
  footer,
  busy = false,
  children
}: ModalProps) {
  const ref = useRef<HTMLDialogElement>(null);
  const bodyRef = useRef<HTMLDivElement>(null);
  const titleId = useId();
  // Closing is a state, not an event: the exit fade needs the element to outlive the decision to
  // close it.
  const [closing, setClosing] = useState(false);
  const exitTimer = useRef<number | undefined>(undefined);
  const focus = useModalFocus(ref, open && !closing);

  // Reconciled every render, so the element converges on React's state however it drifted. The
  // guards keep it idempotent: showModal() on an open dialog throws. `!closing` matters as much as
  // `!open`: without it the render after close() sees `open` still true and re-opens it mid-fade.
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    if (open && !closing && !isOpen(el)) {
      focus.remember();
      // jsdom (the tests) has no showModal; a browser always does.
      if (typeof el.showModal === "function") el.showModal();
      else el.setAttribute("open", "");
      // showModal() focuses the first control, the X, so the dialog would open with Close armed.
      // The body instead: the dialog and its name are still announced, and nothing looks chosen.
      focus.enter(initialFocus?.current, bodyRef.current);
    }
    if ((!open || closing) && isOpen(el)) {
      focus.restore(() => {
        if (typeof el.close === "function") el.close();
        else el.removeAttribute("open");
      });
    }
  });

  const requestClose = useCallback(() => {
    const el = ref.current;
    if (closing || busy) return;
    if (!el || !isOpen(el) || prefersReducedMotion()) {
      onClose();
      return;
    }
    setClosing(true);
    let done = false;
    const finish = () => {
      if (done) return;
      done = true;
      el.removeEventListener("transitionend", onEnd);
      window.clearTimeout(timer);
      exitTimer.current = undefined;
      // Cleared in the same batch as `onClose`, so the next render sees `closing: false` and the
      // parent's `open: false` together; a lone render with `open` true and `closing` false is
      // exactly the one the reconcile effect re-opens on.
      setClosing(false);
      onClose();
    };
    // Only the dialog's OWN opacity: a transition inside it bubbles here too.
    const onEnd = (e: TransitionEvent) => {
      if (e.target === el && e.propertyName === "opacity") finish();
    };
    el.addEventListener("transitionend", onEnd);
    const timer = window.setTimeout(finish, EXIT_FALLBACK_MS);
    exitTimer.current = timer;
  }, [busy, closing, onClose]);

  useEffect(() => () => window.clearTimeout(exitTimer.current), []);

  // The page behind a native modal still scrolls; pin it so a trackpad flick over the backdrop
  // does not move the list out from under the reader.
  useEffect(() => {
    if (!open) return;
    const prev = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    return () => {
      document.body.style.overflow = prev;
    };
  }, [open]);

  return (
    <dialog
      ref={ref}
      aria-labelledby={titleId}
      className={cx("oxy-ui-modal", closing && "oxy-ui-modal--closing")}
      onKeyDown={(e) => {
        if (e.key !== "Escape" || e.defaultPrevented) return;
        e.preventDefault();
        requestClose();
      }}
      // The dialog fills the window and is transparent, so a press on the element itself is a
      // press outside the panel (hit-testing `::backdrop` did not work when measured).
      onClick={(e) => {
        if (e.target === e.currentTarget) requestClose();
      }}
    >
      <div className={cx("oxy-ui-modal__panel", compact && "oxy-ui-modal__panel--compact")}>
        <header className='oxy-ui-modal__head'>
          <h2 id={titleId} className='oxy-ui-modal__title'>
            {title}
          </h2>
          {/* An icon-only action in a panel's head is a text button (decision 47). */}
          <Button
            variant='text'
            iconOnly
            aria-label='Close'
            aria-disabled={busy || undefined}
            icon={<XIcon />}
            className='oxy-ui-modal__close'
            onClick={requestClose}
          />
        </header>
        {/* The body scrolls, not the page; a focus target outside the Tab order. */}
        <div ref={bodyRef} tabIndex={-1} className='oxy-ui-modal__body'>
          {children}
        </div>
        {footer && <footer className='oxy-ui-modal__foot'>{footer}</footer>}
      </div>
    </dialog>
  );
}
