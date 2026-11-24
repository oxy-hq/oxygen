import { useCallback, useState } from "react";

/**
 * Where this browser remembers the admin rail open.
 *
 * The same key shape and the same `"true"` / `"false"` format the product rail uses
 * (`components/Shell/ToggleRail`'s `RAIL_EXPANDED_KEY`), under a name only the admin
 * console reads.
 */
const KEY = "oxy.admin.rail.expanded";

/**
 * The admin rail's open state, remembered per browser — and **not** shared with every
 * other sidebar in the app.
 *
 * ## Why this is not shadcn's cookie
 *
 * `SidebarProvider` writes `sidebar_state` on every toggle. The name is hard-coded in
 * the component, it is written with `path=/`, and every provider in the app writes it:
 * `App.tsx`'s workspace layout, `OrgWorkspace`, `PartnerLayout` and this one. Each also
 * binds ⌘B on `window`. So the cookie is one global "was a sidebar last left open?"
 * flag, not this rail's.
 *
 * The first version of this file read that cookie back, and the leak is real — measured:
 * pressing ⌘B once on `/local/home`, which is the product workspace and has nothing to
 * do with the admin console, set `sidebar_state=false`, and the next load of
 * `/admin/apps` opened the rail as a 3rem strip of marks that nobody had collapsed. A
 * preference you did not set, applied on a surface you were not on.
 *
 * Reading nothing was not an option either — that was the bug before it (a collapse
 * lasting only until the next reload). So the rail is a controlled `SidebarProvider` and
 * its state lives here, under its own key. The provider still writes its cookie when the
 * rail is toggled; nothing reads it, and it is the CLI-managed file's to change.
 *
 * Storage access is guarded on both sides: private-mode Safari throws on the accessor
 * itself, and the read runs in a `useState` initialiser. Unset or unreadable means open,
 * so nobody meets a rail of unlabelled marks they never asked for.
 */
export function useAdminRailOpen(): [open: boolean, setOpen: (open: boolean) => void] {
  const [open, setOpenState] = useState(() => {
    try {
      return localStorage.getItem(KEY) !== "false";
    } catch {
      return true;
    }
  });

  const setOpen = useCallback((next: boolean) => {
    setOpenState(next);
    try {
      localStorage.setItem(KEY, String(next));
    } catch {
      // Private mode or full storage: the choice lasts for this page only.
    }
  }, []);

  return [open, setOpen];
}

export const ADMIN_RAIL_EXPANDED_KEY = KEY;
