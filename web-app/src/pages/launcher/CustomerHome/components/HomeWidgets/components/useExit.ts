import { type TransitionEvent, useCallback, useEffect, useRef, useState } from "react";

/** Longer than the exit's own transition (--duration-fast): the way out if no `transitionend` comes. */
const EXIT_FALLBACK_MS = 300;

/**
 * A card leaving: `leave()` starts its exit motion (`leaving` drives the classes),
 * and `onGone` runs once it has played, which is when the card is really removed.
 * The element that moves spreads `exitProps`. A browser that never runs the
 * transition (a hidden tab, `display: none`) would leave the card half faded, so a
 * timer finishes the exit anyway.
 */
export function useExit(onGone: () => void) {
  const [leaving, setLeaving] = useState(false);
  const finished = useRef(false);
  const finish = useCallback(() => {
    if (finished.current) return;
    finished.current = true;
    onGone();
  }, [onGone]);
  useEffect(() => {
    if (!leaving) return;
    const timer = setTimeout(finish, EXIT_FALLBACK_MS);
    return () => clearTimeout(timer);
  }, [leaving, finish]);
  return {
    leaving,
    leave: () => setLeaving(true),
    exitProps: {
      "data-leaving": leaving || undefined,
      onTransitionEnd: (event: TransitionEvent<HTMLElement>) => {
        if (leaving && event.target === event.currentTarget && event.propertyName === "opacity") {
          finish();
        }
      }
    }
  };
}

/** The motion of a card leaving, on the element `useExit`'s `exitProps` go on. */
export const EXIT_CLASSES =
  "data-[leaving=true]:pointer-events-none data-[leaving=true]:opacity-0 data-[leaving=true]:duration-(--duration-fast) data-[leaving=true]:ease-in data-[leaving=true]:motion-safe:-translate-y-1";
