// Started from oxy-hq/reference-app `src/lib/scrollFocus.ts` and `src/hooks/useScrollFocusable.ts`
// (BEYOND-GALLERY row 19: the reference is tested and measures both axes, which a `DataSheet`
// needs; Store Ops' copy measures across only). Kept: `overflows` and the hook's `{ ref, scrolls }`.
// New: the hook also returns the `tabIndex` to spread, Store Ops' form, so a caller that is not
// DataSheet writes one prop instead of a ternary.
//
// A box that scrolls has to be reachable from the keyboard. A sheet scrolls inside its own box,
// and a box is not a Tab stop of its own: with nothing focusable inside it, a keyboard user can see
// the first columns and never reach the rest (WCAG 2.1.1; axe `scrollable-region-focusable`).
// Chromium has started putting such boxes in the Tab order by itself; WebKit has not, and a box
// with no role and no name is announced as nothing either way.
//
// So the box becomes a Tab stop, where the arrow keys scroll it, but only while it does overflow.
// A table that fits is not one more stop to Tab past. Give it a role and a name while it scrolls
// (`role="region"`, `aria-label`), as DataSheet does.

import { type RefObject, useEffect, useRef, useState } from "react";

/** The four sizes a scroll box reports: its content, and its window onto it. */
export interface ScrollSizes {
  scrollWidth: number;
  clientWidth: number;
  scrollHeight: number;
  clientHeight: number;
}

/**
 * True while there is content past an edge, across or down. The 1px allowance is rounding: a box
 * that fits can report its content a pixel larger than itself.
 */
export function overflows(box: ScrollSizes): boolean {
  return box.scrollWidth > box.clientWidth + 1 || box.scrollHeight > box.clientHeight + 1;
}

export interface ScrollFocusable<T extends HTMLElement> {
  ref: RefObject<T | null>;
  /** The box holds more than it shows. */
  scrolls: boolean;
  /** 0 while it scrolls, none while everything fits. */
  tabIndex: 0 | undefined;
}

/** A ref for a scroll box, whether it scrolls now, and the `tabIndex` it should carry. */
export function useScrollFocusable<T extends HTMLElement>(): ScrollFocusable<T> {
  const ref = useRef<T>(null);
  const [scrolls, setScrolls] = useState(false);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const measure = () => setScrolls(overflows(el));
    measure();
    if (typeof ResizeObserver === "undefined") return;
    // The box resizes with the window; its content with the rows and the folded groups. Both are
    // watched, the content through the box's first child (the table).
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    if (el.firstElementChild) observer.observe(el.firstElementChild);
    return () => observer.disconnect();
  }, []);
  return { ref, scrolls, tabIndex: scrolls ? 0 : undefined };
}
