import { type RefObject, useEffect, useState } from "react";

/**
 * True once `ref`'s element has come within `margin` of the viewport, and true from
 * then on: what holds the app map's trace read until someone scrolls to it. Where
 * `IntersectionObserver` does not exist (jsdom, an old browser) it is true at once,
 * so the panel never waits for an event that cannot come.
 */
export function useSeenOnce(ref: RefObject<HTMLElement | null>, margin = "200px"): boolean {
  const [seen, setSeen] = useState(() => typeof IntersectionObserver === "undefined");
  useEffect(() => {
    const element = ref.current;
    if (seen || !element) return;
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) setSeen(true);
      },
      { rootMargin: margin }
    );
    observer.observe(element);
    return () => observer.disconnect();
  }, [ref, margin, seen]);
  return seen;
}
