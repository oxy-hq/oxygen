// Started from oxy-hq/platform-pulse `src/components/SidePanel.tsx` + `sidePanelFocus.ts` (INVENTORY
// section 2: the 11 real copies are all this modal <dialog>, identical in roadmap and
// accounts-insight; the reference's own SidePanel is a non-modal <aside> with no focus handling,
// behind every copy, and was not used). Kept: the native modal opened before paint, the slide on
// `transform` alone, the scrim and blur fading in with the slide on opacity (round 19's stutter
// fix: nothing at full strength on the first frame), the last content held for the exit, Escape
// unless typing, a press on the page behind, focus to the panel and back to the row that opened
// it, the safe-area padding. New: the look is the reference's panel (card fill, subtle edge) plus
// the overlay shadow, in containers.css; `width` is a CSS length, not a Tailwind class; Tab wraps
// inside (focus/modalFocus.ts); an Escape a layer inside it has taken (a confirm, a hint) is left
// to that layer; and on a phone it is full screen with no scrim. 2.31.0: `id` on the dialog, for
// the `aria-controls` of the rows that open it (pokehouse-oxy Bookkeeping's copy took one).
//
// A right-hand detail panel that slides in and out (decision 41: "panel" is a sheet from the
// right, "modal" a dialog in the centre).
//
// **It overlays rather than sitting in the flow.** As a flex or grid sibling it reflows the table
// beside it on the way in and again on the way out, so the animation is a column of numbers moving
// under the reader's eye while the panel moves. Overlaid, one thing moves and nothing else does.
//
// **It is modal, over the whole window** (decisions 40, 40b, 40c). A native <dialog> opened with
// `showModal()`: it runs the window's whole height, top 0 to the bottom, over the shell's top bar;
// the page behind, top bar included, is dimmed and blurred by the theme's overlay, inert and out of
// the Tab order; focus moves in on open; Escape (except from a field someone is typing in) or a
// press on the page behind asks the caller to close; and on close focus goes back to the control
// that opened it. Another row is opened by closing this one first.
//
// **On a phone** it is the whole screen: nothing of the page shows beside it, so it draws no scrim
// and no blur, and the safe area is padded on every side.
//
// **Exit is the reason this is a component.** Callers render the panel's content from the
// selection, which is null the moment it closes, so this keeps the last content mounted for the
// length of the slide.

import {
  type CSSProperties,
  type ReactNode,
  useEffect,
  useLayoutEffect,
  useRef,
  useState
} from "react";
import { usePhoneLayout } from "../../shell/phone/usePhoneLayout";
import { cx } from "../cx";
import { typingIn, useModalFocus } from "../focus/modalFocus";

/** How long the slide runs, and a little more: the unmount waits this long. */
const SLIDE_MS = 300;

function prefersReducedMotion(): boolean {
  return (
    typeof window !== "undefined" &&
    typeof window.matchMedia === "function" &&
    window.matchMedia("(prefers-reduced-motion: reduce)").matches
  );
}

/** Close a dialog opened modal, or opened by attribute where there is no `showModal`. */
function closeDialog(d: HTMLDialogElement | null): void {
  if (!d?.open && !d?.hasAttribute("open")) return;
  if (typeof d.close === "function") d.close();
  else d.removeAttribute("open");
}

export interface SidePanelProps {
  open: boolean;
  /** On the dialog, for the `aria-controls` of the rows that open it. */
  id?: string;
  /** The panel's accessible name. */
  label: string;
  /** The panel's widest, a CSS length (`"560px"`, `"34rem"`); never wider than the window. Panels
   *  do not agree on one, and should not. Default 560px. Ignored on a phone (full width). */
  width?: string;
  /** On the panel itself (its fill, padding, a layout of its own). */
  className?: string;
  style?: CSSProperties;
  /** Escape, or a press on the page behind: the caller closes the panel. */
  onClose: () => void;
  children: ReactNode;
}

export function SidePanel({
  open,
  id,
  label,
  width,
  className,
  style,
  onClose,
  children
}: SidePanelProps) {
  const phone = usePhoneLayout();
  const [mounted, setMounted] = useState(open);
  // Mounted in the same render that opens it, so the dialog is there to open.
  if (open && !mounted) setMounted(true);
  /** Drives the slide and the scrim: false for one committed frame after mount, so there is
   *  something to transition FROM. */
  const [shown, setShown] = useState(false);
  const last = useRef<ReactNode>(null);
  if (open) last.current = children;
  const dialog = useRef<HTMLDialogElement>(null);
  const panel = useRef<HTMLDivElement>(null);
  const focus = useModalFocus(dialog, open && mounted);
  const closeRef = useRef(onClose);
  closeRef.current = onClose;

  useEffect(() => {
    if (open) return;
    setShown(false);
    const finish = () => {
      // Back to the control that opened it: the browser's own restore after close() goes to
      // whatever had focus before, which after a click on a row's cells is the scroller around
      // the table, not the row.
      focus.restore(() => closeDialog(dialog.current));
      setMounted(false);
    };
    if (prefersReducedMotion()) {
      finish();
      return;
    }
    const timer = window.setTimeout(finish, SLIDE_MS);
    return () => window.clearTimeout(timer);
  }, [open, focus]);

  // Opened as a modal before paint.
  useLayoutEffect(() => {
    const d = dialog.current;
    if (!mounted || !open || !d) return;
    if (!d.open && !d.hasAttribute("open")) {
      focus.remember();
      // jsdom (the tests) has no showModal; a browser always does.
      if (typeof d.showModal === "function") d.showModal();
      else d.setAttribute("open", "");
      // showModal() runs the dialog's focusing steps: the panel itself (`tabIndex=-1`, the first
      // focusable area inside) takes focus, and Tab reaches the first control next. Where it did
      // not (jsdom, an engine with no showModal), focus is moved to the panel here.
      if (!d.contains(document.activeElement)) focus.enter(panel.current);
    }
    if (!shown) {
      // Not requestAnimationFrame: rAF is throttled to nothing in a background tab, which left
      // the reference's first version mounted and off screen. Reading layout forces the closed
      // transform to be computed before the flip.
      panel.current?.getBoundingClientRect();
      setShown(true);
    }
  }, [mounted, open, shown, focus]);

  if (!mounted) return null;

  return (
    <dialog
      ref={dialog}
      id={id}
      aria-label={label}
      data-testid='side-panel'
      className={cx(
        "oxy-ui-side-panel",
        shown && "oxy-ui-side-panel--shown",
        !open && "oxy-ui-side-panel--closing",
        phone && "oxy-ui-side-panel--phone"
      )}
      // Escape is ours, not the browser's, so the slide runs and a field keeps its own Escape. An
      // Escape a layer inside took (a confirm over the panel, a hint) is that layer's.
      onKeyDown={(e) => {
        if (e.key !== "Escape" || e.defaultPrevented) return;
        e.preventDefault();
        if (!typingIn(e.target)) closeRef.current();
      }}
      onCancel={(e) => {
        e.preventDefault();
        closeRef.current();
      }}
      // The dialog fills the window and is transparent, so a press that lands on it, not on the
      // panel, is a press on the page behind.
      onClick={(e) => {
        if (e.target === e.currentTarget) closeRef.current();
      }}
    >
      <div
        ref={panel}
        tabIndex={-1}
        className={cx("oxy-ui-side-panel__panel", className)}
        style={width ? ({ "--oxy-ui-side-panel-width": width, ...style } as CSSProperties) : style}
      >
        {open ? children : last.current}
      </div>
    </dialog>
  );
}
