import { type RefObject, useEffect, useRef, useState } from "react";

/**
 * Moving between the browser's tiers, and keeping the deepest one in view.
 *
 * **← and → move focus; they do not select.** Diving into a tier to look at it should
 * not commit a choice, which on the Apps tier means mounting a console. Finder selects
 * on →; here that would cost a page load for a glance, so the first row of an unvisited
 * tier is simply a tab stop (`ColumnPane`) and ↓ is what selects.
 *
 * Focus is moved in an effect rather than inline because the tier being focused may not
 * have rendered its new tab stop yet when the key is handled.
 *
 * The scroll is the thing that makes a column browser work at any width instead of
 * silently hiding its right-hand end: `nearest` so a tier already on screen does not
 * move, and so nothing scrolls vertically. It only runs when the rail actually overflows.
 */
export function useRailFocus(depths: readonly number[]): {
  railRef: RefObject<HTMLDivElement | null>;
  moveFocus: (from: number, delta: -1 | 1) => void;
} {
  const railRef = useRef<HTMLDivElement>(null);
  const [wanted, setWanted] = useState<number | null>(null);

  useEffect(() => {
    if (wanted === null) return;
    railRef.current
      ?.querySelector(`[data-browser-depth="${wanted}"]`)
      ?.querySelector<HTMLElement>('[role="option"][tabindex="0"]')
      ?.focus();
    setWanted(null);
  }, [wanted]);

  const deepest = depths.at(-1);
  useEffect(() => {
    const rail = railRef.current;
    if (!rail || deepest === undefined) return;
    const show = () => {
      if (rail.scrollWidth <= rail.clientWidth) return;
      rail
        .querySelector(`[data-browser-depth="${deepest}"]`)
        ?.scrollIntoView({ inline: "nearest", block: "nearest" });
    };
    show();
    // **Also on resize, which is not a detail.** The rail only scrolls once it is wider
    // than its room, so narrowing the window is exactly when the tier you are working in
    // goes off the left — measured at 1280px: the Pages tier sat entirely outside the
    // rail with nothing bringing it back. Running this only when the tier set changes
    // left that case uncovered.
    const observer = new ResizeObserver(show);
    observer.observe(rail);
    return () => observer.disconnect();
  }, [deepest]);

  return {
    railRef,
    moveFocus: (from, delta) => {
      const at = depths.indexOf(from);
      const next = at < 0 ? undefined : depths[at + delta];
      if (next !== undefined) setWanted(next);
    }
  };
}
