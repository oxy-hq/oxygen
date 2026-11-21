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
//
// 2.31.0 (the gap round):
//   - A closed `<details>` hides everything in it but its own summary, so none of it is a Tab
//     stop: Platform Pulse's area panel ended on a link in a folded "How it is answered", Tab from
//     the real last control left the dialog, and the trap never wrapped (measured, Tab 21 of 40
//     on <body>).
//   - `useModalFocus(ref, open, { auto: true })` for a dialog an app draws itself (BMG Inventory
//     Management's seven `.review-backdrop` sheets): it remembers the opener, moves focus in,
//     traps Tab and gives focus back, with no calls of its own. A dialog that mounts open and
//     focuses a field itself (`autoFocus`) has moved focus in before any effect runs, so the
//     opener is the last thing focused outside it, which this module now keeps.

import { type RefObject, useEffect, useLayoutEffect, useRef } from "react";

/** What can take focus by Tab inside a modal. */
export const MODAL_FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]):not([type="hidden"]), select:not([disabled]), textarea:not([disabled]), summary, [tabindex]:not([tabindex="-1"])';

/** Whether a closed `<details>` hides `el`: everything in one is hidden but its own summary (and
 *  what is in that summary), and that details must itself be shown. */
function inClosedDetails(el: HTMLElement): boolean {
  for (let node: Element = el; ; ) {
    const details = node.parentElement?.closest("details");
    if (!details) return false;
    if (!details.open) {
      const summary = details.querySelector(":scope > summary");
      if (!summary?.contains(el)) return true;
    }
    node = details;
  }
}

/** The Tab stops inside `root`, in document order: none inside a hidden or inert part, none
 *  inside a nested dialog that is not open, and none a closed `<details>` hides. */
export function focusableIn(root: HTMLElement): HTMLElement[] {
  return [...root.querySelectorAll<HTMLElement>(MODAL_FOCUSABLE)].filter((el) => {
    if (el.closest("[hidden], [inert]")) return false;
    if (inClosedDetails(el)) return false;
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
let pressedAt = 0;
// The last few elements focused, newest last: a dialog that mounts and focuses a field of its own
// has taken focus before it can ask what had it.
const focused: { el: HTMLElement; at: number }[] = [];
let tick = 0;
if (typeof document !== "undefined") {
  document.addEventListener(
    "pointerdown",
    (e) => {
      pressed = e.target instanceof Element ? e.target : null;
      pressedAt = ++tick;
    },
    true
  );
  document.addEventListener(
    "focusin",
    (e) => {
      if (!(e.target instanceof HTMLElement)) return;
      focused.push({ el: e.target, at: ++tick });
      if (focused.length > 8) focused.shift();
    },
    true
  );
}

/** The last element focused outside `root`, and when, if it is still on the page. */
function focusedBefore(root: HTMLElement): { el: HTMLElement; at: number } | null {
  for (let i = focused.length - 1; i >= 0; i--) {
    const f = focused[i];
    if (f.el.isConnected && !root.contains(f.el) && f.el !== document.body) return f;
  }
  return null;
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
  /** Call as the modal opens: remembers what opened it. Focus already inside the modal (a field
   *  it focused itself) is passed over for what had focus before it, or the press that opened it. */
  remember(): void;
  /** Moves focus in: to the first of `targets` that takes it. False when none did. */
  enter(...targets: (HTMLElement | null | undefined)[]): boolean;
  /** Gives focus back to what opened the modal, if focus is inside it or nowhere. `close`, given,
   *  runs between that check and the return: it closes the native dialog, whose own restore (to
   *  whatever had focus before, often the scroller a click on a cell focused) would otherwise
   *  stand. */
  restore(close?: () => void): void;
}

export interface ModalFocusOptions {
  /**
   * For a dialog drawn by hand (not a SidePanel, Modal or ConfirmDialog): the hook does the whole
   * job. As `isOpen` turns true (or the dialog mounts open) it remembers the opener and, if
   * nothing inside has focus, moves focus to the first control (or the dialog itself); while open
   * Tab stays inside; as `isOpen` turns false, or the dialog unmounts, focus goes back.
   */
  auto?: boolean;
}

/**
 * Focus for a modal whose element is `root`: while `open`, Tab stays inside it (when it is the
 * top-most); `remember` / `enter` / `restore` move focus in and give it back; and an unmount
 * while open gives it back too (a dialog removed from the document hands focus to no one). With
 * `{ auto: true }` it calls them itself (`ModalFocusOptions`).
 */
export function useModalFocus(
  root: RefObject<HTMLElement | null>,
  isOpen: boolean,
  options: ModalFocusOptions = {}
): ModalFocus {
  const opener = useRef<HTMLElement | null>(null);
  const api = useRef<ModalFocus>(null as unknown as ModalFocus);
  if (!api.current) {
    api.current = {
      remember() {
        const active = document.activeElement;
        const box = root.current;
        if (box && active instanceof Node && box.contains(active)) {
          // Focus is already inside: the dialog took it on mount. The opener is what had it just
          // before, unless a press came after that (Safari focuses no button on a click).
          const before = focusedBefore(box);
          opener.current =
            before && before.at > pressedAt
              ? openerFor(before.el)
              : (openerFor(null) ?? before?.el ?? null);
          return;
        }
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

  // `auto`: in a layout effect, before the browser paints the open dialog; its cleanup runs as
  // the dialog closes or unmounts, while focus is still inside it.
  const auto = options.auto === true;
  useLayoutEffect(() => {
    const el = root.current;
    if (!auto || !isOpen || !el) return;
    const focus = api.current;
    focus.remember();
    if (!el.contains(document.activeElement)) focus.enter(focusableIn(el)[0], el);
    return () => focus.restore();
  }, [auto, isOpen, root]);

  return api.current;
}
