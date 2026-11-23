// Where the nav sheet stands in for the rail, in its own module so both the sheet
// (`NavSheet.tsx`) and the rail (`ShellRail.tsx`) can read it: the sheet already imports
// the rail's mark, and the rail reading the query back from the sheet would close a cycle.

import { useSyncExternalStore } from "react";

/** Where the drawer stands in for the rail: below 768px, and on a phone on its
 *  side (the phone layout's landscape half), whose 390px cannot hold the rail's
 *  items at a 44px touch size. Kept equal to the query in shell.css. */
export const NAV_SHEET_QUERY = "(max-width: 767.98px), (pointer: coarse) and (max-height: 500px)";

function navSheetQuery(): MediaQueryList | null {
  if (typeof window === "undefined" || typeof window.matchMedia !== "function") return null;
  return window.matchMedia(NAV_SHEET_QUERY);
}

function subscribe(onChange: () => void): () => void {
  const query = navSheetQuery();
  query?.addEventListener?.("change", onChange);
  return () => query?.removeEventListener?.("change", onChange);
}

const getSnapshot = () => navSheetQuery()?.matches ?? false;
// No window (server render): the desk, which a phone then replaces on its first client render.
const getServerSnapshot = () => false;

/** True under NAV_SHEET_QUERY. Re-renders when the answer changes (rotating the
 *  phone, resizing the window). */
export function useNavSheetLayout(): boolean {
  return useSyncExternalStore(subscribe, getSnapshot, getServerSnapshot);
}
