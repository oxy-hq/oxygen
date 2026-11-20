// Started from pokehouse-oxy `apps/supply-chain/src/hooks/useRetryFocus.ts` (INVENTORY section 5:
// the same code as warehouse's and bookkeeping's, ahead of Marketing's original: it handles a
// re-read that keeps the callout up a moment, and phone kit rows). Kept whole: the names, the
// holder rule, the hand-on from the outermost part that left. Changed: the page it falls back to
// lists the shell's main first, and the no-ring rule for a lent target is focus.css's.
//
// Where keyboard focus goes after Try again (decisions 14 and 28).
//
// Try again lives inside the failed read's callout (`LoadError`), and the re-read replaces that
// callout: first with the loading bar, then with the content, or with a new callout if it fails
// again. The button that had focus is gone at the first step, so focus fell to <body>: a screen
// reader lost its place and the next Tab started over. The reference has that defect: its
// `useReread` remounts the part that reads, and its `LoadError` goes with it.
//
// So focus is handed on, never dropped:
//   in flight    the loading bar or line takes it (it says what is being read; it is
//                `tabIndex=-1`, so never a Tab stop, and draws no ring);
//   success      the element that took the removed part's place takes it, the start of the new
//                content, so the next Tab continues from there;
//   fails again  the new callout's Try again takes it.
// Nothing moves unless a Try again started it, and a hand-on happens only while focus is still
// where the last step put it: a person who tabbed or clicked away keeps their place.
//
// A re-read does not always draw its loading line in the same render as Try again (a read that
// failed may go back to loading in its own effect), so the callout can stay up, focused, while the
// read starts. The step that holds focus is remembered (`holder`) until it hands on or the person
// moves.

import { type RefObject, useLayoutEffect } from "react";

/** The element holding focus for a Try again in progress, or null. */
let holder: HTMLElement | null = null;
/** `holder` is where a hand-on put focus (the start of what took the callout's place), not a Try
 *  again or a loading line. */
let handedOn = false;
/** A Try again or loading line whose part has just gone while it held focus: React can keep its
 *  element for what replaces it (a kit row reused for the next row), so focus never drops to
 *  <body>, and what replaces it may take focus as if it had. */
let leaving: HTMLElement | null = null;
let listening = false;

function listen(): void {
  if (listening || typeof document === "undefined") return;
  listening = true;
  // The person moved: focus went somewhere none of these steps put it, or they pressed somewhere.
  // Their place wins from then on.
  document.addEventListener(
    "focusin",
    (e) => {
      if (holder && e.target !== holder) holder = null;
    },
    true
  );
  document.addEventListener(
    "pointerdown",
    () => {
      holder = null;
    },
    true
  );
}

/** Focus dropped to <body> because the step holding it has just been removed: the next loading
 *  line or Try again to appear takes it. One that appears while the holder is still up (a second
 *  callout, a second loading line in a closed section) leaves it where it is. */
function dropped(): boolean {
  if (!holder || holder.isConnected) return false;
  const a = document.activeElement;
  return !a || a === document.body;
}

/** Marks an element that only receives focus from here: no ring (focus.css). */
const TARGET = "data-retry-focus";

/** Focus `el`, made focusable (`tabIndex=-1`, no ring) for as long as it holds focus if it is not
 *  already. False if it could not take focus (not rendered, inside a closed section). */
function focusQuietly(el: HTMLElement): boolean {
  const lent = el.tabIndex < 0 && !el.hasAttribute("tabindex");
  const giveBack = () => {
    el.removeAttribute("tabindex");
    el.removeAttribute(TARGET);
  };
  if (lent) {
    el.setAttribute("tabindex", "-1");
    el.setAttribute(TARGET, "");
  }
  el.focus({ preventScroll: true });
  const took = document.activeElement === el;
  if (lent) {
    if (took) el.addEventListener("blur", giveBack, { once: true });
    else giveBack();
  }
  return took;
}

function take(el: HTMLElement): void {
  holder = el;
  handedOn = false;
  focusQuietly(el);
}

/** A new Try again may take focus: it dropped to <body> with the step holding it, it is on a step
 *  that has just gone but whose element React kept (`leaving`), or it is still where a hand-on put
 *  it. The last is a re-read that draws no loading line and fails again: its callout went, focus
 *  went on to what took its place, and the new callout's Try again takes it back. */
function claimable(): boolean {
  return (
    dropped() || (!!holder && document.activeElement === holder && (handedOn || holder === leaving))
  );
}

/** Where the page's content starts, the last place to hand focus to. */
const PAGE = ".oxy-shell__main, main, .oxy-phone-page__body";

/** `el` is going: once React has committed, if no new loading line or Try again took focus, give
 *  it to whatever took the removed part's place. That part is usually wider than `el` (the
 *  callout's row, the card a loading bar sits in), so the place is found from `el`'s ancestors: the
 *  outermost one that left the page, and the sibling it came after. */
function handOnFrom(el: HTMLElement): void {
  if (holder !== el || document.activeElement !== el) return;
  leaving = el;
  const chain: { node: HTMLElement; prev: Element | null }[] = [];
  for (
    let n: HTMLElement | null = el;
    n && n !== document.body && chain.length < 16;
    n = n.parentElement
  ) {
    chain.push({ node: n, prev: n.previousElementSibling });
  }
  queueMicrotask(() => {
    if (leaving === el) leaving = null;
    if (holder !== el) return;
    holder = null;
    const a = document.activeElement;
    if (a && a !== document.body) return;
    const gone = chain.findIndex(
      (c, i) => !c.node.isConnected && (chain[i + 1]?.node.isConnected ?? true)
    );
    let next: Element | null = null;
    if (gone >= 0) {
      const parent = chain[gone + 1]?.node ?? null;
      const prev = chain[gone].prev;
      next =
        prev?.isConnected && prev.parentElement === parent
          ? prev.nextElementSibling
          : (parent?.firstElementChild ?? null);
      // Nothing took its place in its own box (the callout was the head's last line): what
      // follows the box is where the new content is.
      for (
        let p: HTMLElement | null = parent;
        !next && p && p !== document.body;
        p = p.parentElement
      )
        next = p.nextElementSibling;
    }
    // The first of these that can take focus: the new content, else the part around it (a target
    // inside a closed section cannot), else the page.
    const page = document.querySelector<HTMLElement>(PAGE);
    const candidates: HTMLElement[] = [];
    for (
      let to = next as HTMLElement | null;
      to && to !== document.body;
      to = to === page ? null : to.parentElement
    )
      candidates.push(to);
    if (page) candidates.push(page);
    for (const to of candidates) {
      holder = to;
      handedOn = true;
      if (focusQuietly(to)) return;
    }
    holder = null;
  });
}

/** Try again was pressed (`button`, or what has focus): its read's next loading line, content or
 *  callout takes focus. */
export function markRetry(button?: HTMLElement | null): void {
  if (typeof document === "undefined") return;
  listen();
  const active = document.activeElement;
  const el = button ?? (active instanceof HTMLElement && active !== document.body ? active : null);
  holder = el;
  handedOn = false;
}

/** What a ref points at, or for a kit part (which takes no ref) the element `find` reaches from a
 *  span of ours inside it. Pass one of the two below, not a new function each render: a new one
 *  re-runs the effect. */
export type RetryFocusFind = (inner: HTMLElement) => HTMLElement | null;
/** The kit button (`PhoneButton`, a `PhoneRow` that is a button) around a span. */
export const kitButton: RetryFocusFind = (inner) => inner.closest("button");
/** The kit row (`PhoneRow`) around a span. */
export const kitRow: RetryFocusFind = (inner) => inner.closest<HTMLElement>(".oxy-phone-row");
const resolve = (ref: RefObject<HTMLElement | null>, find?: RetryFocusFind) =>
  ref.current && find ? find(ref.current) : ref.current;

/**
 * A Try again (or, with `find`, the kit button around `ref`). When it appears because a re-read
 * failed again, it takes focus; when it goes while it still holds focus (the read worked), focus
 * goes to what took its place.
 */
export function useRetryButtonFocus(
  button: RefObject<HTMLElement | null>,
  find?: RetryFocusFind
): void {
  useLayoutEffect(() => {
    const el = resolve(button, find);
    if (!el) return;
    if (claimable()) take(el);
    return () => handOnFrom(el);
  }, [button, find]);
}

/**
 * A loading line (`tabIndex=-1`, `data-retry-focus`; or, with `find`, the kit row around `ref`):
 * holds focus while a re-read runs, then hands it to whatever takes its place.
 */
export function useHoldFocusDuringRetry(
  line: RefObject<HTMLElement | null>,
  find?: RetryFocusFind
): void {
  useLayoutEffect(() => {
    const el = resolve(line, find);
    if (!el) return;
    if (dropped() || (holder === leaving && document.activeElement === holder)) take(el);
    return () => handOnFrom(el);
  }, [line, find]);
}
