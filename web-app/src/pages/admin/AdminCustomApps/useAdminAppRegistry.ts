import { useEffect, useMemo } from "react";
import { ADMIN_APP_PAGE_SIZE, useAdminApps } from "@/hooks/api/customApps/useCustomApps";
import type { CustomApp } from "@/types/apps";

/**
 * The whole custom-app registry, plus whichever app the `:orgSlug/:appSlug`
 * route segments point at.
 *
 * All pages are loaded up front so filter / sort / group operate over the whole
 * registry rather than just the first page — admin scale is dozens to low
 * hundreds, so a handful of background fetches is cheap. Revisit with
 * server-side querying only if the registry ever grows into the thousands.
 *
 * Shared by the fleet list, an app's console, the preview stage and the popped-out
 * dossier window, so all four resolve an app the same way. The window is a separate browsing context with its own React
 * tree and QueryClient, so it does NOT share the opener's in-memory cache — it
 * fetches the registry independently (fine at the dozens-to-low-hundreds scale
 * this list targets; a by-slug lookup would avoid the walk if it ever isn't).
 */
export function useAdminAppRegistry(orgSlug?: string, appSlug?: string) {
  const { data, isLoading, error, refetch, hasNextPage, isFetchingNextPage, fetchNextPage } =
    useAdminApps(ADMIN_APP_PAGE_SIZE);

  const apps = useMemo(() => data?.pages.flatMap((p) => p.items) ?? [], [data]);

  // Walk the remaining pages automatically so callers see every app.
  //
  // `!error` stops the walk once a page has genuinely failed. Without it this loops
  // forever: React Query gives up after its 3 retries, `isFetchingNextPage` drops to
  // false, `hasNextPage` is still true (it is recomputed from the last *successful*
  // page, whose `next_offset` is non-null), so the effect re-fires and starts the whole
  // retry cycle again. Measured against a page 2 that always 500s: 18 requests in 30
  // seconds, on a ~7s cycle, with no end. The Retry button on the error state is how a
  // walk resumes after that — a human asking, rather than a loop assuming.
  useEffect(() => {
    if (hasNextPage && !isFetchingNextPage && !error) fetchNextPage({ cancelRefetch: false });
  }, [hasNextPage, isFetchingNextPage, fetchNextPage, error]);

  const selectedKey = useMemo(
    () => (orgSlug && appSlug ? `${orgSlug}/${appSlug}` : null),
    [orgSlug, appSlug]
  );

  const selected = useMemo<CustomApp | null>(
    () =>
      selectedKey ? (apps.find((a) => `${a.org_slug}/${a.slug}` === selectedKey) ?? null) : null,
    [apps, selectedKey]
  );

  return {
    apps,
    selected,
    selectedKey,
    isLoading,
    /**
     * True until every page has been walked.
     *
     * Callers must not conclude "no such app" from `selected === null` while this is
     * set: `isLoading` covers only the FIRST page, so a deep link to an app on page 2
     * of a >100-app registry otherwise renders "It may have been deleted" — a
     * confident false claim. `isFetchingNextPage` alone is not enough, because it
     * drops to false for a tick between pages.
     */
    isWalking: hasNextPage || isFetchingNextPage,
    error,
    refetch
  };
}
