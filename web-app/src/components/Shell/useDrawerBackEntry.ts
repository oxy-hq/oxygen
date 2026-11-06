import { useCallback, useEffect, useRef, useState } from "react";

// What makes the phone's Back close the drawer. Open state alone has no history
// entry, so Back moved the page under the drawer and left the drawer open. So an
// open drawer holds one entry on the same URL: Back steps off it, the page stays.
//
// The entry keeps the router's own state (its index and key), so the router reads
// the step off it as a move to where it already is.

const MARK = "oxyNavDrawer";
/** Longer than any real traversal; a `back` that lands nowhere never fires popstate. */
const MOVE_TIMEOUT_MS = 1000;

const onOurEntry = () => (window.history.state as Record<string, unknown> | null)?.[MARK] === true;

function pushEntry() {
  const state = { ...(window.history.state as object | null), [MARK]: true };
  window.history.pushState(state, "", window.location.href);
}

export function useDrawerBackEntry() {
  const [open, setOpenState] = useState(false);
  const openRef = useRef(false);
  /** Our own `back` is on its way; `then` is what waits for it to land. */
  const move = useRef<{ then?: () => void; timer: ReturnType<typeof setTimeout> } | null>(null);
  const checkedLoad = useRef(false);

  const landed = useCallback(() => {
    const finished = move.current;
    if (!finished) return;
    clearTimeout(finished.timer);
    move.current = null;
    // Opened again while the last entry was going away: it gets a new one.
    if (openRef.current) pushEntry();
    finished.then?.();
  }, []);

  const stepOff = useCallback(
    (then?: () => void) => {
      move.current = { then, timer: setTimeout(landed, MOVE_TIMEOUT_MS) };
      window.history.back();
    },
    [landed]
  );

  const show = useCallback(() => {
    openRef.current = true;
    setOpenState(true);
    if (!move.current) pushEntry();
  }, []);

  /** X, overlay, Escape, a pick: take the entry away. `then` runs once it has gone,
   *  so a navigation it starts lands on the page's entry, not on top of ours. */
  const hide = useCallback(
    (then?: () => void) => {
      openRef.current = false;
      setOpenState(false);
      // Not on our entry: the page navigated from inside the drawer. Going back
      // would undo that, so ours stays behind, and is stepped off if Back reaches it.
      if (onOurEntry() && !move.current) stepOff(then);
      else then?.();
    },
    [stepOff]
  );

  /** Leave for another document: its load takes our entry's place in the history,
   *  since a traversal under way would cancel it. */
  const hideAndReplace = useCallback((href: string) => {
    openRef.current = false;
    setOpenState(false);
    window.location.replace(href);
  }, []);

  useEffect(() => {
    const onPopState = () => {
      if (move.current) return landed();
      // Any step of the history closes the drawer: nothing in it jumps to a hash.
      openRef.current = false;
      setOpenState(false);
      // Back onto one left behind, or Forward onto a closed drawer's: a step that
      // would show no change, so keep going down.
      if (onOurEntry()) stepOff();
    };
    window.addEventListener("popstate", onPopState);
    // A reload with the drawer open comes back closed, on its entry.
    if (!checkedLoad.current) {
      checkedLoad.current = true;
      if (onOurEntry()) stepOff();
    }
    return () => window.removeEventListener("popstate", onPopState);
  }, [landed, stepOff]);

  useEffect(
    () => () => {
      if (move.current) clearTimeout(move.current.timer);
    },
    []
  );

  return { open, show, hide, hideAndReplace };
}
