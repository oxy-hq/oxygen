// Prefetch the app a person is about to open from the dock.
//
// The rail is the dock: every app entry is a plain anchor that takes this tab
// to the next app. A Speculation Rules prefetch on hover (`moderate`
// eagerness) fetches that app's page before the click, so the browser can
// show it without another round trip, and the response's preload hints pull
// its entry chunks too. Chrome and Edge only; elsewhere the app's service
// worker is what keeps a repeat open fast. Only where the opened page can tell
// it came from a prefetch (`deliveryType` on its navigation entry), because
// that is how its open gets counted; see below.
//
// Document rules, so it follows the rail as apps come and go without wiring a
// handler per entry. Same-origin links only (`href_matches: "/*"`): in local
// dev the rail points at the Oxygen origin, and a cross-site prefetch would go
// without cookies and only waste a request. Never the current app.
//
// A prefetch, never a prerender: a prerender would run the app, its queries
// and its functions, for a hover. The serve path records no open for the
// prefetch (`Sec-Purpose`), and the opened page reports its own (`runtime.js`).

import { useEffect } from "react";

export const DOCK_PREFETCH_RULES = {
  prefetch: [
    {
      where: {
        and: [
          { href_matches: "/*" },
          { selector_matches: ".oxy-rail a.oxy-rail__item[href]:not([aria-current='page'])" }
        ]
      },
      eagerness: "moderate"
    }
  ]
} as const;

/** Speculation Rules, in a browser whose opened page can tell it was one. */
function supportsSpeculationRules(): boolean {
  return (
    typeof HTMLScriptElement !== "undefined" &&
    typeof HTMLScriptElement.supports === "function" &&
    HTMLScriptElement.supports("speculationrules") &&
    typeof PerformanceNavigationTiming !== "undefined" &&
    "deliveryType" in PerformanceNavigationTiming.prototype
  );
}

/** Install the dock's prefetch rules for as long as the shell is mounted. */
export function useDockPrefetch(): void {
  useEffect(() => {
    if (!supportsSpeculationRules()) return;
    const script = document.createElement("script");
    script.type = "speculationrules";
    script.textContent = JSON.stringify(DOCK_PREFETCH_RULES);
    document.head.appendChild(script);
    return () => script.remove();
  }, []);
}
