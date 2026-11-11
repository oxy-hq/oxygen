// The frame of a phone view. It does not scroll by itself: the page grows and
// the scroller around it (the shell's main area, or the document) scrolls it,
// so there is never a fixed-height box inside a box. The tab bar and sticky
// action sit in a dock at the end of the page, pinned to the bottom of the
// screen with `position: sticky`. In flow, the content always clears them, and
// they span only the app's own column (beside the shell rail when a phone on
// its side shows it, beside an open Ask dock).

import { type ReactNode, type RefObject, useEffect, useLayoutEffect, useRef } from "react";
import { cx } from "../cx";
import { forgetScroll } from "./layers";
import { fillHeight, nearestScroller } from "./scroller";
import { present } from "./tone";

export interface PhonePageProps {
  children: ReactNode;
  /** A `PhoneTabBar`. */
  tabBar?: ReactNode;
  /** A `PhoneStickyAction`, shown above the tab bar when both are given. */
  stickyAction?: ReactNode;
  /** When it changes (e.g. the active tab's id), the page scrolls to its top. */
  scrollKey?: string | number;
}

// `min-height: 100%` in shell.css reaches the scroller only when the page sits
// directly in it. Under a wrapper that hugs its content it resolves to nothing,
// and a short page's dock would sit mid-screen, so the height is measured.
function useFillScroller(ref: RefObject<HTMLDivElement | null>, enabled: boolean) {
  useLayoutEffect(() => {
    const page = ref.current;
    if (!enabled || !page) return;
    const measure = () => {
      page.style.minHeight = "";
      const height = fillHeight(page);
      if (height > 0) page.style.minHeight = `${height}px`;
    };
    measure();
    // A frame later: measuring inside the observer's own callback changes the
    // sizes it watches, which browsers report to window.onerror as a loop.
    let frame = 0;
    const schedule = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(measure);
    };
    window.addEventListener("resize", schedule);
    // What is above the page can change size too (an app header loading).
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(schedule);
    for (let node = page.parentElement; node && observer; node = node.parentElement) {
      observer.observe(node);
    }
    return () => {
      cancelAnimationFrame(frame);
      window.removeEventListener("resize", schedule);
      observer?.disconnect();
      page.style.minHeight = "";
    };
  }, [ref, enabled]);
}

export function PhonePage({ children, tabBar, stickyAction, scrollKey }: PhonePageProps) {
  const ref = useRef<HTMLDivElement>(null);
  const lastKey = useRef(scrollKey);

  useEffect(() => {
    if (lastKey.current === scrollKey) return;
    lastKey.current = scrollKey;
    const scroller = nearestScroller(ref.current);
    // A sheet closing in the same commit (the More list choosing a tab) would put
    // the old tab's position back a frame later.
    forgetScroll(scroller);
    scroller.scrollTop = 0;
  }, [scrollKey]);

  const docked = present(tabBar) || present(stickyAction);
  useFillScroller(ref, docked);
  return (
    <div
      ref={ref}
      data-testid='phone-page'
      className={cx(
        "oxy-shell-scope oxy-phone oxy-phone-page",
        present(tabBar) && "oxy-phone-page--tabbar",
        !docked && "oxy-phone-page--bare"
      )}
    >
      <div className='oxy-phone-page__body'>{children}</div>
      {docked && (
        <div className='oxy-phone-page__dock'>
          {stickyAction}
          {tabBar}
        </div>
      )}
    </div>
  );
}
