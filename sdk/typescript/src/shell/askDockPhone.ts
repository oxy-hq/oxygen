// Ask Oxygen on a phone. Docked beside the app, as on a desktop, the panel took
// min(23rem, 85vw) of a 390px screen and left the app a ~60px column, its text
// wrapping a letter a line. In the phone layout the open panel covers the whole
// screen instead (`.oxy-askdock--phone` in shell.css): out of the flex row, so the
// app keeps its width and is not reflowed, and inert behind it. The phone's Back
// closes it (one history entry, as the Today panel and a PhoneSheet hold), and the
// panel follows the visual viewport, so the composer sits above the keyboard. A
// tablet and a desktop keep the docked side panel: none of this runs there.

import { type RefObject, useEffect, useRef } from "react";
import { lockScroll } from "./phone/layers";
import { holdHistoryEntry } from "./phone/sheetHistory";
import { usePhoneLayout } from "./phone/usePhoneLayout";

/** The visible viewport's height and top, which a keyboard shrinks and scrolls. */
function fitToViewport(dock: HTMLElement): () => void {
  const view = window.visualViewport;
  if (!view) return () => {};
  const measure = () => {
    dock.style.setProperty("--oxy-askdock-vh", `${view.height}px`);
    dock.style.setProperty("--oxy-askdock-top", `${view.offsetTop}px`);
  };
  measure();
  dock.setAttribute("data-fit", "");
  view.addEventListener("resize", measure);
  view.addEventListener("scroll", measure);
  return () => {
    view.removeEventListener("resize", measure);
    view.removeEventListener("scroll", measure);
    dock.style.removeProperty("--oxy-askdock-vh");
    dock.style.removeProperty("--oxy-askdock-top");
    dock.removeAttribute("data-fit");
  };
}

/** Everything beside the dock in the shell (rail, app column, open sheets) goes inert. */
function inertBeside(dock: HTMLElement): () => void {
  const others = [...(dock.parentElement?.children ?? [])].filter(
    (el) => el !== dock && !el.hasAttribute("inert")
  );
  for (const el of others) el.setAttribute("inert", "");
  return () => {
    for (const el of others) el.removeAttribute("inert");
  };
}

/** True in the phone layout, where the open dock covers the screen. */
export function useAskDockPhone(
  dock: RefObject<HTMLElement | null>,
  open: boolean,
  onClose: () => void
): boolean {
  const phone = usePhoneLayout();
  const closeRef = useRef(onClose);
  closeRef.current = onClose;
  const covering = phone && open;

  useEffect(() => {
    if (covering) return holdHistoryEntry(() => closeRef.current());
  }, [covering]);

  useEffect(() => {
    const el = dock.current;
    if (!covering || !el) return;
    const unlock = lockScroll();
    const release = inertBeside(el);
    const unfit = fitToViewport(el);
    return () => {
      unfit();
      release();
      unlock();
    };
  }, [covering, dock]);

  return phone;
}
