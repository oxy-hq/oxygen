import { useCallback, useState } from "react";

const KEY = "oxy.admin.appBrowser.railCollapsed";

/**
 * Whether this browser hides the Organizations and Apps tiers.
 *
 * **Not in the URL, on purpose.** A pasted `/admin/apps/acme/store-ops/data` link has to
 * open the same page for whoever receives it; how wide the person who sent it likes
 * their nav is not part of what they were pointing at. It is a view preference, so it
 * lives per browser, like the agent-skills reminder's dismissal.
 *
 * Storage access is guarded on both sides: private-mode Safari throws on the accessor
 * itself, and this runs in a `useState` initialiser.
 */
export function useRailCollapsed(): [collapsed: boolean, toggle: () => void] {
  const [collapsed, setCollapsed] = useState(() => {
    try {
      return localStorage.getItem(KEY) === "1";
    } catch {
      return false;
    }
  });
  const toggle = useCallback(() => {
    setCollapsed((was) => {
      const next = !was;
      try {
        localStorage.setItem(KEY, next ? "1" : "0");
      } catch {
        /* the rail comes back expanded next visit; nothing worse */
      }
      return next;
    });
  }, []);
  return [collapsed, toggle];
}
