import { useEffect, useState } from "react";
import { flushSync } from "react-dom";

const key = (orgSlug: string) => `oxy.home.today.${orgSlug}`;

function read(orgSlug: string | undefined): boolean | null {
  if (!orgSlug) return null;
  try {
    const value = window.localStorage.getItem(key(orgSlug));
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
  const [hint, setHint] = useState<boolean | null>(() => read(orgSlug));
  useEffect(() => setHint(read(orgSlug)), [orgSlug]);
  useEffect(() => {
    if (!orgSlug || known === undefined) return;
    try {
      window.localStorage.setItem(key(orgSlug), known ? "1" : "0");
    } catch {
      // Private mode or full storage: the next visit just has no hint.
    }
  }, [orgSlug, known]);
  return known ?? hint;
}

/**
 * `value`, but a change to it is applied inside a view transition, so the
 * page's layout change animates: when an org's first widget arrives, each app
 * tile glides from its place in four columns to its place beside the Today rail
 * instead of jumping. Plain where the browser has no view transitions or the
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
