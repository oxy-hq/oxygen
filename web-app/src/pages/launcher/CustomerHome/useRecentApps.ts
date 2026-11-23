import { useCallback, useMemo, useState } from "react";
import type { CustomAppSummary } from "@/types/apps";

/** App ids, the most recently opened first. */
type Recent = string[];

/** How many are remembered unless the caller asks for more. */
const KEEP = 4;

const key = (orgSlug: string) => `oxy.home.recent.${orgSlug}`;

function read(orgSlug: string, keep: number): Recent {
  try {
    const raw = window.localStorage.getItem(key(orgSlug));
    const ids: unknown = raw ? JSON.parse(raw) : [];
    return Array.isArray(ids)
      ? ids.filter((id): id is string => typeof id === "string").slice(0, keep)
      : [];
  } catch {
    return [];
  }
}

/**
 * The apps this person last opened from this org's home in this browser, newest
 * first, and `opened` to record one. Apps they can no longer open drop out.
 * `keep` is how many are remembered: the home orders every app by it
 * ("Recently used"), so it asks for many.
 */
export function useRecentApps(
  orgSlug: string | undefined,
  apps: readonly CustomAppSummary[],
  keep: number = KEEP
) {
  // Bumped on each open, so the list is read again.
  const [version, setVersion] = useState(0);
  const recent = useMemo(() => {
    void version;
    if (!orgSlug) return [];
    const byId = new Map(apps.map((app) => [app.id, app]));
    return read(orgSlug, keep).flatMap((id) => byId.get(id) ?? []);
  }, [orgSlug, apps, version, keep]);

  const opened = useCallback(
    (app: CustomAppSummary) => {
      if (!orgSlug) return;
      const next = [app.id, ...read(orgSlug, keep).filter((id) => id !== app.id)].slice(0, keep);
      try {
        window.localStorage.setItem(key(orgSlug), JSON.stringify(next));
      } catch {
        // Private mode or full storage: no recent row next time.
      }
      setVersion((v) => v + 1);
    },
    [orgSlug, keep]
  );

  return { recent, opened };
}
