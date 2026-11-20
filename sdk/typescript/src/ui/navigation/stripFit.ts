// Started from pokehouse-oxy:apps/store-ops/src/chrome/stripFit.ts (the only copy; its two
// importers, DefaultTabs and RadioSegments, are the SDK's DefaultTabs and Segmented, and Store
// Ops' DefaultTabs tests cover it). Kept as it is: one measurement of the strip as drawn, a
// swap to a native select once its options are wider than its room, and back once the room
// returns. New: `optionText` takes the count Store Ops' DefaultTabs gave it, unchanged.
//
// When a segmented strip stops fitting, it becomes a select (decisions 22 and 29). The
// reference's guidance (`views/Patterns.tsx`): "The strip does not wrap: with many or long
// options it outgrows a narrow container, which is the sign it should be a select." It used to
// scroll sideways inside itself, hiding the options past its edge. This hook measures the strip
// as it is drawn (no second copy, so a strip that fits renders exactly as before) and, once its
// options are wider than the room it has, swaps it for a native select until the room comes
// back.

import { useLayoutEffect, useRef, useState } from "react";

/** The width inside an element's padding. */
function innerWidth(el: HTMLElement): number {
  const style = getComputedStyle(el);
  return (
    el.clientWidth - parseFloat(style.paddingLeft || "0") - parseFloat(style.paddingRight || "0")
  );
}

/**
 * Whether a strip fits its room, measured as it is drawn.
 *
 * Put `strip` on the strip and `select` on the select drawn in its place while `fits` is false.
 * `watch` changes when the options do (their labels, their counts), which measures again.
 * `enabled` false: the strip is never swapped (a strip that scrolls instead, `DefaultTabs`'
 * `overflow="scroll"`), and nothing is measured.
 */
export function useStripFits<T extends HTMLElement>(watch: unknown, enabled = true) {
  const strip = useRef<T>(null);
  const select = useRef<HTMLSelectElement>(null);
  // The width the strip needs, once it did not fit; null while it is drawn.
  const [need, setNeed] = useState<number | null>(null);
  const room = useRef<HTMLElement | null>(null);

  // The strip is drawn: does it fit?
  // biome-ignore lint/correctness/useExhaustiveDependencies: `watch` is the options; new options measure again.
  useLayoutEffect(() => {
    if (!enabled || need !== null) return;
    const el = strip.current;
    if (!el) return;
    const check = () => {
      if (el.scrollWidth > el.clientWidth + 1) {
        room.current = el.parentElement;
        setNeed(el.scrollWidth);
      }
    };
    check();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(check);
    ro.observe(el);
    if (el.parentElement) ro.observe(el.parentElement);
    return () => ro.disconnect();
  }, [enabled, need, watch]);

  // The select is drawn: has the room come back?
  useLayoutEffect(() => {
    if (need === null) return;
    const parent = room.current ?? select.current?.parentElement ?? null;
    if (!parent) return;
    const check = () => {
      if (innerWidth(parent) >= need) setNeed(null);
    };
    check();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(check);
    ro.observe(parent);
    return () => ro.disconnect();
  }, [need]);

  return { strip, select, fits: !enabled || need === null };
}

/** An option's label as plain words for a select, with its count when it has one (a 0 too: the
 *  strip draws it, so the select says it). */
export function optionText(label: string, count?: number | null): string {
  return typeof count === "number" ? `${label} (${count})` : label;
}
