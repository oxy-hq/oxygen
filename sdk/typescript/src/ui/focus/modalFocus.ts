// Started from oxy-hq/platform-pulse `src/components/sidePanelFocus.ts` (INVENTORY section 5: 11
// identical copies, the modal side panel's helper: which control opened it, and whether Escape
// belongs to a field), with the trap of oxy-hq/bmg-industries-oxy
// `apps/reorder-sheet/src/modalFocus.ts` (INVENTORY: written because Tab walked out of a dialog onto
// the dimmed page, and on close focus fell to <body>).
//
// Kept: `openerFor` (the pressed row's own control, not the scroller a click on a cell focuses) and
// `typingIn`, both unchanged. New: the trap and the return as one hook, `useModalFocus`, so the
// SidePanel, the Modal and the ConfirmDialog move focus in, hold it and give it back one way.
//
// **Why a trap on top of `showModal()`.** A native modal makes the page inert, so Tab cannot reach
// it; but from the last control Tab still leaves for the browser's own toolbar and comes back at
// the top of the dialog. Wrapping at the two ends keeps Tab and Shift+Tab inside, as the phone
// kit's sheets do (`shell/NavSheet.tsx` `trapTab`). Only the top-most open modal traps, so a
// confirm opened over a side panel holds focus and the panel does not fight it.
//
// **The return.** Focus goes back to the control that opened the modal when it closes or is
// unmounted, if focus is still inside it or has fallen to <body>. A caller that moved focus
// somewhere on purpose (a refusal line, the next row) keeps it there.

import { type RefObject, useEffect, useRef } from "react";

/** What can take focus by Tab inside a modal. */
export const MODAL_FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]):not([type="hidden"]), select:not([disabled]), textarea:not([disabled]), summary, [tabindex]:not([tabindex="-1"])';

/** The Tab stops inside `root`, in document order: none inside a hidden or inert part, and none
 *  inside a nested dialog that is not open. */
export function focusableIn(root: HTMLElement): HTMLElement[] {
  return [...root.querySelectorAll<HTMLElement>(MODAL_FOCUSABLE)].filter((el) => {
    if (el.closest("[hidden], [inert]")) return false;
    const dialog = el.closest("dialog");
    return !dialog || dialog === root || dialog.hasAttribute("open");
  });
}

/** Tab and Shift+Tab wrap inside `root`; a Tab from outside it lands on its first stop. */
export function trapTab(event: KeyboardEvent, root: HTMLElement | null): void {
  if (!root || event.key !== "Tab") return;
  const stops = focusableIn(root);
  const first = stops[0];
  const last = stops[stops.length - 1];
  const active = document.activeElement;
  if (!first || !last) {
    // Nothing to move to: focus stays where it is (the panel itself).
    event.preventDefault();
    return;
  }
  const inside = active instanceof Node && root.contains(active);
  if (event.shiftKey && (!inside || active === first || active === root)) {
    event.preventDefault();
    last.focus();
  } else if (!event.shiftKey && (!inside || active === last)) {
    event.preventDefault();
    first.focus();
  }
}

// The last element a pointer pressed. A row opened by a click on its cells (not on its button)
// leaves focus on <body>; the row's own control is where focus goes back to.
let pressed: Element | null = null;
if (typeof document !== "undefined") {
  document.addEventListener(
    "pointerdown",
    (e) => {
      pressed = e.target instanceof Element ? e.target : null;
    },
    true
  );
}

/** The control a pointer press opened the modal from: the pressed link or button, else the
 *  pressed row's first control. */
function fromPress(): HTMLElement | null {
  if (!pressed?.isConnected) return null;
  const row = pressed.closest('tr, [role="row"], li');
  const own = pressed.closest<HTMLElement>(MODAL_FOCUSABLE);
  if (own && (!row || row.contains(own))) return own;
  return row?.querySelector<HTMLElement>(MODAL_FOCUSABLE) ?? own ?? null;
}

/** The control that opened the modal: the focused one, or, after a click on a row's cells, the
 *  row's own control. A click on a cell can also focus a scroller around the table (a sheet is a
 *  Tab stop): the row's control is the one to go back to then, not the scroller. */
export function openerFor(active: Element | null): HTMLElement | null {
  const press = fromPress();
  if (active instanceof HTMLElement && active !== document.body) {
    if (press && press !== active && active.contains(press)) return press;
    return active;
  }
  return press;
}

/** Escape stays with a field someone is typing in (the search box, a note). */
export function typingIn(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null;
  const tag = el?.tagName;
  return (
    tag === "TEXTAREA" || tag === "INPUT" || tag === "SELECT" || el?.isContentEditable === true
  );
}

/** The open modals, top-most last: only the top one traps Tab. */
const open: HTMLElement[] = [];

/** True when `el` is the top-most open modal. */
export function isTopModal(el: HTMLElement | null): boolean {
  return !!el && open[open.length - 1] === el;
}

export interface ModalFocus {
  /** Call as the modal opens, before `showModal()` moves focus: remembers what opened it. */
  remember(): void;
  /** Moves focus in: to the first of `targets` that takes it. False when none did. */
  enter(...targets: (HTMLElement | null | undefined)[]): boolean;
  /** Gives focus back to what opened the modal, if focus is inside it or nowhere. `close`, given,
   *  runs between that check and the return: it closes the native dialog, whose own restore (to
   *  whatever had focus before, often the scroller a click on a cell focused) would otherwise
   *  stand. */
  restore(close?: () => void): void;
}

/**
 * Focus for a modal whose element is `root`: while `open`, Tab stays inside it (when it is the
 * top-most); `remember` / `enter` / `restore` move focus in and give it back; and an unmount
 * while open gives it back too (a dialog removed from the document hands focus to no one).
 */
export function useModalFocus(root: RefObject<HTMLElement | null>, isOpen: boolean): ModalFocus {
  const opener = useRef<HTMLElement | null>(null);
  const api = useRef<ModalFocus>(null as unknown as ModalFocus);
  if (!api.current) {
    api.current = {
      remember() {
        const active = document.activeElement;
        if (root.current && active instanceof Node && root.current.contains(active)) return;
        opener.current = openerFor(active);
      },
      enter(...targets) {
        for (const to of targets) {
          if (!to?.isConnected) continue;
          to.focus({ preventScroll: true });
          if (document.activeElement === to) return true;
        }
        return false;
      },
      restore(close) {
        const back = opener.current;
        opener.current = null;
        const active = document.activeElement;
        const box = root.current;
        const lost =
          !active ||
          active === document.body ||
          active === document.documentElement ||
          !!box?.contains(active);
        close?.();
        if (lost && back?.isConnected) back.focus({ preventScroll: true });
      }
    };
  }

  useEffect(() => {
    const el = root.current;
    if (!isOpen || !el) return;
    open.push(el);
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Tab" && isTopModal(el)) trapTab(e, el);
    };
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("keydown", onKey);
      const at = open.lastIndexOf(el);
      if (at >= 0) open.splice(at, 1);
    };
  }, [isOpen, root]);

  useEffect(() => {
    const focus = api.current;
    return () => focus.restore();
  }, []);

  return api.current;
}
