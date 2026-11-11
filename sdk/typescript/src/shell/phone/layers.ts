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

interface Spot {
  top: number;
  left: number;
  /** The page it belongs to: the URL, less its hash. */
  url: string;
}
interface Held extends Spot {
  el: HTMLElement;
  overflow: string;
}
let held: Held[] = [];
/** Follow-up frames still holding a scroller where it was, by scroller. */
const pending = new Map<HTMLElement, number>();
/** How long after a release the position is held: longer than a Back's traversal
 *  takes to land (sheetHistory's MOVE_TIMEOUT_MS). */
const SETTLE_MS = 1000;
/** A person's own scroll or press, which ends any hold: the page is theirs again. */
const INPUT = ["pointerdown", "touchstart", "wheel", "keydown"];
let lastInput = 0;
/** When a history traversal last landed: the browser may restore a scroll then. */
let lastPopstate = 0;

// The document, and the shell's main area, which is the page's scroller when
// the app runs inside <OxyShell>.
function scrollers(): HTMLElement[] {
  const found = [document.documentElement, document.body];
  for (const main of document.querySelectorAll<HTMLElement>(".oxy-shell__main")) found.push(main);
  return found;
}

/** Stop putting `el` back where a lock found it: something has moved it on purpose
 *  (a new tab scrolling to its top). */
export function forgetScroll(el: HTMLElement): void {
  const frame = pending.get(el);
  if (frame !== undefined) cancelAnimationFrame(frame);
  pending.delete(el);
}

/**
 * Put a scroller back where the lock found it, and hold it there for a moment.
 * The lock alone cannot promise that. A box keeps its scrollTop only while its
 * content is tall enough, so a list that renders shorter under the sheet (a
 * loading state, fewer rows for a while) is clamped to the top and stays there
 * when it grows back. And a Back lands on the page's entry after the sheet has
 * gone, where the browser restores the document to whatever it saved when the
 * sheet's entry was pushed: the clamped top again. So for a second after the
 * release each frame puts the box back, as long as only those two moved it: a
 * clamp (the box at its end, short of the position) or, for the document, a
 * traversal landing.
 * Anything else that scrolls it (the person, the app bringing a saved row into
 * view, a new page by URL) ends the hold, and its scroll stands.
 */
function putBack({ el, top, left, url }: Held): void {
  // Another page by now (a link followed from inside the sheet): the position was
  // this one's, and that page starts where its own navigation put it.
  if (!el.isConnected || url !== pageUrl()) return;
  let expected = { top: el.scrollTop, left: el.scrollLeft };
  const apply = () => {
    if (el.scrollTop !== top) el.scrollTop = top;
    if (el.scrollLeft !== left) el.scrollLeft = left;
    expected = { top: el.scrollTop, left: el.scrollLeft };
  };
  apply();
  if (typeof requestAnimationFrame === "undefined") return;
  const from = Date.now();
  let seenPop = lastPopstate;
  const ours = () => {
    if (el.scrollTop === expected.top && el.scrollLeft === expected.left) return true;
    // Only the document is restored by a traversal; a box the browser never
    // restores was moved by someone else.
    const landed = lastPopstate !== seenPop && isDocument(el);
    seenPop = lastPopstate;
    return landed || clampedShort(el, top, left);
  };
  const step = () => {
    pending.delete(el);
    if (!el.isConnected || lastInput >= from || Date.now() - from > SETTLE_MS) return;
    if (url !== pageUrl() || !ours()) return;
    apply();
    pending.set(el, requestAnimationFrame(step));
  };
  pending.set(el, requestAnimationFrame(step));
}

const isDocument = (el: Element) =>
  el === document.documentElement || el === document.scrollingElement;

/** `el` sits at its end, short of (top, left): its content cannot hold them yet.
 *  To the pixel, since a high-density screen scrolls by fractions. */
function clampedShort(el: HTMLElement, top: number, left: number): boolean {
  const cut = (want: number, now: number, end: number) =>
    want === now || (want > now && now >= end - 1);
  return (
    cut(top, el.scrollTop, el.scrollHeight - el.clientHeight) &&
    cut(left, el.scrollLeft, el.scrollWidth - el.clientWidth)
  );
}

const pageUrl = () => window.location.href.split("#")[0];

// Where each scroller was at its last scroll event. The sheet's lock runs after
// the commit that opened it, and a list that commit made shorter has already been
// clamped by then; the clamp's own scroll event comes a frame later, so this still
// holds the position the person was looking at.
const seen = new WeakMap<Element, Spot>();

if (typeof document !== "undefined") {
  for (const type of INPUT) {
    document.addEventListener(
      type,
      () => {
        lastInput = Date.now();
      },
      { capture: true, passive: true }
    );
  }
  window.addEventListener(
    "popstate",
    () => {
      lastPopstate += 1;
    },
    { capture: true }
  );
  document.addEventListener(
    "scroll",
    (e) => {
      const el = e.target === document ? document.scrollingElement : e.target;
      if (el instanceof Element) {
        seen.set(el, { top: el.scrollTop, left: el.scrollLeft, url: pageUrl() });
      }
    },
    { capture: true, passive: true }
  );
}

/**
 * Where `el` stands for the person: where they left it, if since then it has only
 * been clamped by content too short to hold that position. With the page they
 * left it on, which a sheet the URL names has already moved off by the time it
 * locks.
 */
function standing(el: HTMLElement): Spot {
  const now = { top: el.scrollTop, left: el.scrollLeft, url: pageUrl() };
  const was = seen.get(el);
  if (!was) return now;
  if (was.top === now.top && was.left === now.left) return was;
  return clampedShort(el, was.top, was.left) ? was : now;
}

/** Stop the page behind from scrolling; returns the release. Nested locks
 *  release together when the last one goes, and the release puts the page back
 *  exactly where the first lock found it. */
export function lockScroll(): () => void {
  if (locks === 0) {
    for (const el of [...pending.keys()]) forgetScroll(el);
    held = scrollers().map((el) => {
      const hold = { el, overflow: el.style.overflow, ...standing(el) };
      el.style.overflow = "hidden";
      return hold;
    });
  }
  locks += 1;
  let released = false;
  return () => {
    if (released) return;
    released = true;
    locks -= 1;
    if (locks === 0) {
      const was = held;
      held = [];
      // Every overflow first: a box still `hidden` around one put back would cut it short.
      for (const hold of was) hold.el.style.overflow = hold.overflow;
      for (const hold of was) putBack(hold);
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
