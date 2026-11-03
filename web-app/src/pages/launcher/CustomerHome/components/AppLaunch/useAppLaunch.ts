import type { MouseEvent } from "react";
import { useCallback, useEffect, useRef, useState } from "react";
import { flushSync } from "react-dom";
import type { CustomAppSummary } from "@/types/apps";
import { appFor, clearHandoff, leaveHandoff } from "./handoff";

/** The view-transition name the pressed tile and the launch screen share, so one
 *  grows into the other. */
const LAUNCH_NAME = "app-launch";

/** A navigation still not landed after this long is not coming: give the home back. */
const GIVE_UP_MS = 20_000;

/** Where a launch grew from, and the transition name it had before it. */
type Origin = { el: HTMLElement; name: string };

/** Run `update` as a view transition, with `direction` on `<html>` for the CSS
 *  that tells opening from closing (`motion.css`, "An app opening from the home"). */
function morph(direction: "open" | "close", update: () => void) {
  const html = document.documentElement;
  html.dataset.appLaunch = direction;
  const done = () => {
    if (html.dataset.appLaunch === direction) delete html.dataset.appLaunch;
  };
  const transition = document.startViewTransition(update);
  transition.finished.then(done, done);
  return transition;
}

function canMorph(): boolean {
  return (
    typeof document.startViewTransition === "function" &&
    !window.matchMedia("(prefers-reduced-motion: reduce)").matches
  );
}

/** A plain press: not a new tab, a new window or a download, which leave the home showing. */
function plainPress(event: MouseEvent): HTMLAnchorElement | null {
  if (event.defaultPrevented || event.button !== 0) return null;
  if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return null;
  const anchor = (event.target as Element | null)?.closest?.("a[href]");
  if (!(anchor instanceof HTMLAnchorElement)) return null;
  if ((anchor.target && anchor.target !== "_self") || anchor.hasAttribute("download")) return null;
  return anchor;
}

/**
 * Opening an app from the home, the way a phone opens one from its home screen:
 * the pressed tile (or widget) grows into the app's launch screen, its icon and
 * name on the home's ground, and that screen stays while the app loads. The app's
 * runtime then draws the same screen as its first frame and lifts it once the app
 * has drawn itself (`handoff.ts`), so a slow first load reads as the app starting,
 * not as a frozen page. A bar along the top appears only if the wait runs long,
 * the way a desktop browser shows progress.
 *
 * Nothing waits on the animation: the link navigates as it always did, and the
 * browser keeps this page painted until the app's first frame. Back restores the
 * home from the back-forward cache, where the launch screen shrinks back into
 * the tile it came from.
 *
 * `onClick` goes on a container: any link inside it that opens one of `apps`
 * launches, whichever tile, card or row it is. The element marked
 * `data-app-launch-origin` around the link is what grows.
 */
export function useAppLaunch(apps: readonly CustomAppSummary[]) {
  const [launching, setLaunching] = useState<CustomAppSummary | null>(null);
  // Whether the launch screen grew from the tile, or has to fade in on its own.
  const [grown, setGrown] = useState(false);
  const origin = useRef<Origin | null>(null);
  const appsNow = useRef(apps);
  appsNow.current = apps;

  const close = useCallback(() => {
    clearHandoff();
    const from = origin.current;
    origin.current = null;
    const restore = () => {
      if (from) from.el.style.viewTransitionName = from.name;
    };
    if (!from?.el.isConnected || !canMorph()) {
      setLaunching(null);
      restore();
      return;
    }
    const shrink = morph("close", () => {
      flushSync(() => setLaunching(null));
      from.el.style.viewTransitionName = LAUNCH_NAME;
    });
    shrink.finished.then(restore, restore);
  }, []);

  const onClick = useCallback((event: MouseEvent) => {
    const anchor = plainPress(event);
    if (!anchor) return;
    const opens = appFor(anchor.href, appsNow.current);
    if (!opens) return;
    const el = anchor.closest<HTMLElement>("[data-app-launch-origin]") ?? anchor;
    leaveHandoff(opens.app, opens.root, el.querySelector("[data-slot=app-mark]"));
    origin.current = { el, name: el.style.viewTransitionName };
    setGrown(canMorph());
    if (!canMorph()) {
      setLaunching(opens.app);
      return;
    }
    // The tile carries the shared name into the "before" picture and hands it to
    // the launch screen in the "after" one.
    el.style.viewTransitionName = LAUNCH_NAME;
    morph("open", () => {
      flushSync(() => setLaunching(opens.app));
      el.style.viewTransitionName = "none";
    });
  }, []);

  useEffect(() => {
    if (!launching) return;
    // Back from the app restores this page as it was left: launching.
    const onPageShow = (event: PageTransitionEvent) => {
      if (event.persisted) close();
    };
    // Escape stops the page load in a browser; the launch screen goes with it.
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") close();
    };
    const timer = window.setTimeout(close, GIVE_UP_MS);
    window.addEventListener("pageshow", onPageShow);
    window.addEventListener("keydown", onKey);
    return () => {
      window.clearTimeout(timer);
      window.removeEventListener("pageshow", onPageShow);
      window.removeEventListener("keydown", onKey);
    };
  }, [launching, close]);

  return { launching, grown, onClick, launchName: LAUNCH_NAME };
}
