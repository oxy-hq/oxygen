// Shared by every full-screen phone layer (PhoneSheet, and the tab bar's
// More sheet, which is one): a stack so Escape closes only the top layer, and
// a scroll lock so the page behind does not move while any layer is open.

import { FOCUSABLE } from "../NavSheet";

const stack: symbol[] = [];

/** Put a layer on top; returns the function that removes it. */
export function pushLayer(id: symbol): () => void {
  stack.push(id);
  return () => {
    const at = stack.lastIndexOf(id);
    if (at >= 0) stack.splice(at, 1);
  };
}

export function isTopLayer(id: symbol): boolean {
  return stack[stack.length - 1] === id;
}

let locks = 0;
let restore: Array<() => void> = [];

// The document, and the shell's main area, which is the page's scroller when
// the app runs inside <OxyShell>.
function scrollers(): HTMLElement[] {
  const found = [document.documentElement, document.body];
  for (const main of document.querySelectorAll<HTMLElement>(".oxy-shell__main")) found.push(main);
  return found;
}

/** Stop the page behind from scrolling; returns the release. Nested locks
 *  release together when the last one goes. */
export function lockScroll(): () => void {
  if (locks === 0) {
    restore = scrollers().map((el) => {
      const before = el.style.overflow;
      el.style.overflow = "hidden";
      return () => {
        el.style.overflow = before;
      };
    });
  }
  locks += 1;
  let released = false;
  return () => {
    if (released) return;
    released = true;
    locks -= 1;
    if (locks === 0) {
      for (const undo of restore) undo();
      restore = [];
    }
  };
}

// Safari focuses nothing when a button is tapped, so `activeElement` cannot say
// what opened a sheet. The control under the last pointer press can. Listening
// from load, because an app may mount a sheet only once it is open, after the press.
const RECENT_MS = 2000;
let pressed: Element | null = null;
let pressedAt = 0;

if (typeof document !== "undefined") {
  document.addEventListener(
    "pointerdown",
    (e) => {
      pressed = e.target instanceof Element ? e.target : null;
      pressedAt = Date.now();
    },
    { capture: true, passive: true }
  );
}

/** What focus should go back to: the focused element, else the control just pressed. */
export function focusOrigin(): HTMLElement | null {
  const active = document.activeElement;
  if (active instanceof HTMLElement && active !== document.body) return active;
  if (!pressed || Date.now() - pressedAt > RECENT_MS) return null;
  return pressed.closest<HTMLElement>(FOCUSABLE);
}
