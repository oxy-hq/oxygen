import { useEffect, useState } from "react";
import { flushSync } from "react-dom";

/** What the home remembers about its own frame, per org: whether it had a Today, and
 *  whether it had a side column. */
type FramePart = "today" | "side";

const key = (part: FramePart, orgSlug: string) => `oxy.home.${part}.${orgSlug}`;

function read(part: FramePart, orgSlug: string | undefined): boolean | null {
  if (!orgSlug) return null;
  try {
    const value = window.localStorage.getItem(key(part, orgSlug));
    return value === "1" ? true : value === "0" ? false : null;
  } catch {
    return null;
  }
}

/**
 * Whether this org's home had a Today the last time this browser drew it, so
 * the first paint already has the right frame: an org without widgets never
 * sees a rail flash in and the apps reflow from two columns to four, and one
 * with widgets has its rail (showing a skeleton) before they load.
 *
 * `known` is the answer once the widgets have loaded; it is remembered for next
 * time. `null` while neither this browser nor the server has said.
 */
export function useTodayHint(orgSlug: string | undefined, known: boolean | undefined) {
  return useFrameHint("today", orgSlug, known);
}

/**
 * Whether this org's home had a side column the last time this browser drew it.
 * The side column holds Today, recent questions and the data map, and the last two
 * load after the apps: without the hint, the apps would paint full width and then
 * reflow into the narrower grid on every visit. Same contract as `useTodayHint`.
 */
export function useSideHint(orgSlug: string | undefined, known: boolean | undefined) {
  return useFrameHint("side", orgSlug, known);
}

function useFrameHint(part: FramePart, orgSlug: string | undefined, known: boolean | undefined) {
  const [hint, setHint] = useState<boolean | null>(() => read(part, orgSlug));
  useEffect(() => setHint(read(part, orgSlug)), [part, orgSlug]);
  useEffect(() => {
    if (!orgSlug || known === undefined) return;
    try {
      window.localStorage.setItem(key(part, orgSlug), known ? "1" : "0");
    } catch {
      // Private mode or full storage: the next visit just has no hint.
    }
  }, [part, orgSlug, known]);
  return known ?? hint;
}

/**
 * `value`, but a change to it is applied inside a view transition, so the
 * page's layout change animates: when an org's first widget arrives, Today eases
 * into the side column and what was there moves down, or, where the side column
 * is new, each app tile glides to its narrower grid instead of jumping. Plain where the browser has no view transitions or the
 * person asked for reduced motion.
 */
export function useTransitionedFlag(value: boolean): boolean {
  const [shown, setShown] = useState(value);
  useEffect(() => {
    if (value === shown) return;
    const reduced = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? true;
    if (typeof document.startViewTransition === "function" && !reduced) {
      // The callback runs outside this effect, so the DOM must update inside it.
      document.startViewTransition(() => flushSync(() => setShown(value)));
    } else {
      setShown(value);
    }
  }, [value, shown]);
  return shown;
}
