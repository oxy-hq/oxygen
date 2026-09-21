import { useEffect, useMemo } from "react";
import { useAdminApps } from "@/hooks/api/customApps/useCustomApps";
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
 * Shared by the cockpit and the popped-out dossier window so both resolve an
 * app the same way. The window is a separate browsing context with its own React
 * tree and QueryClient, so it does NOT share the opener's in-memory cache — it
 * fetches the registry independently (fine at the dozens-to-low-hundreds scale
 * this list targets; a by-slug lookup would avoid the walk if it ever isn't).
 */
export function useAdminAppRegistry(orgSlug?: string, appSlug?: string) {
  const { data, isLoading, error, refetch, hasNextPage, isFetchingNextPage, fetchNextPage } =
    useAdminApps(100);

  const apps = useMemo(() => data?.pages.flatMap((p) => p.items) ?? [], [data]);

  // Walk the remaining pages automatically so callers see every app.
  useEffect(() => {
    if (hasNextPage && !isFetchingNextPage) fetchNextPage();
  }, [hasNextPage, isFetchingNextPage, fetchNextPage]);

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
    isLoadingMore: isFetchingNextPage,
    error,
    refetch
  };
}
