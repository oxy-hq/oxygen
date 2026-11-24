import { useEffect, useMemo, useRef, useState } from "react";
import { useLocation, useNavigate, useSearchParams } from "react-router-dom";
import type { CustomApp } from "@/types/apps";
import type { AppsRollup } from "../../appBrowserModel";
import {
  appKey,
  appsInScope,
  type BrowserScope,
  type OrgRow,
  orgRows,
  rollup,
  scopeFrom
} from "../../appBrowserModel";
import type { HealthIndex } from "../../appStatus";
import { type ConsolePageId, consolePagePath } from "../AppConsole/consolePages";
import { ALL_ORGS } from "./OrgsColumn";
import type { SelectHow } from "./useColumnKeys";

/** How long a keyboard move waits before it commits. Long enough to scan a column,
 *  short enough that one deliberate ↓ does not feel held back. */
const COMMIT_MS = 180;

export interface BrowserSelection {
  orgs: OrgRow[];
  everything: AppsRollup;
  scope: BrowserScope;
  scopedApps: CustomApp[];
  /** The Apps tier's highlight, which runs ahead of the URL while the keyboard moves. */
  highlightedKey: string | null;
  selectOrg: (id: string, how: SelectHow) => void;
  selectApp: (key: string, how: SelectHow) => void;
  selectPage: (id: ConsolePageId, how: SelectHow) => void;
}

/**
 * What the browser's three tiers select, and what selecting does to the URL.
 *
 * Everything here is derived from the registry already on the page, so the tiers cost no
 * request. The URLs do not change shape: an app is still
 * `/admin/apps/:orgSlug/:appSlug/:consolePage`, and `?org=` is the one addition, which
 * means anything only while no app is selected.
 *
 * Three rules worth knowing:
 *
 * **A click pushes history; a keyboard move replaces it.** Arrowing down a column of
 * forty apps should not leave forty entries behind the Back button.
 *
 * **Arrowing through apps is debounced; the highlight is not.** Every committed app
 * mounts a console, and each of its pages runs its own queries — holding ↓ through forty
 * apps would mount forty consoles and fire every request each one makes. So a keyboard
 * move paints at once and commits when the keys stop. A click commits immediately, being
 * one deliberate choice rather than a scan.
 *
 * **Selecting an app keeps the page you are on.** That is the trip this whole redesign
 * exists to shorten: walking down the Apps tier with Activity open walks you through
 * every app's Activity, instead of landing on each overview and clicking through.
 */
export function useBrowserSelection(
  apps: CustomApp[],
  selected: CustomApp | null,
  health: HealthIndex | undefined,
  page: ConsolePageId | null
): BrowserSelection {
  const navigate = useNavigate();
  const { search } = useLocation();
  const [params] = useSearchParams();

  const orgs = useMemo(() => orgRows(apps, health), [apps, health]);
  const everything = useMemo(() => rollup(apps, health), [apps, health]);
  const scope = useMemo(
    () => scopeFrom(selected, params.get("org"), orgs),
    [selected, params, orgs]
  );
  const scopedApps = useMemo(() => appsInScope(apps, scope), [apps, scope]);

  const committedKey = selected ? appKey(selected) : null;
  const [pending, setPending] = useState<string | null>(null);

  const goToApp = (key: string, how: SelectHow) => {
    const next = scopedApps.find((a) => appKey(a) === key);
    if (!next) return;
    navigate(consolePagePath(next, page ?? "overview", search), { replace: how === "key" });
  };

  // The ref is what keeps `pending` the only dependency: re-running the timer on `page`
  // or `search` would restart it mid-scan, which is the one thing it must not do.
  const goToAppRef = useRef(goToApp);
  goToAppRef.current = goToApp;
  useEffect(() => {
    if (pending === null) return;
    const timer = setTimeout(() => {
      goToAppRef.current(pending, "key");
      setPending(null);
    }, COMMIT_MS);
    return () => clearTimeout(timer);
  }, [pending]);

  return {
    orgs,
    everything,
    scope,
    scopedApps,
    highlightedKey: pending ?? committedKey,
    selectOrg: (id, how) => {
      setPending(null);
      // Selecting a parent drops the deeper selection, the way selecting a folder does.
      const to = id === ALL_ORGS ? "/admin/apps" : `/admin/apps?org=${encodeURIComponent(id)}`;
      navigate(to, { replace: how === "key" });
    },
    selectApp: (key, how) => {
      if (how === "key") {
        setPending(key);
        return;
      }
      setPending(null);
      goToApp(key, how);
    },
    selectPage: (id, how) => {
      if (!selected) return;
      navigate(consolePagePath(selected, id, search), { replace: how === "key" });
    }
  };
}
