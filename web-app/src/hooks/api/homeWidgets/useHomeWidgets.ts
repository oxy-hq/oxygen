import { useQuery } from "@tanstack/react-query";
import { HomeWidgetsService } from "@/services/api/homeWidgets";
import type { HomeSection, HomeWidgetsResponse } from "@/types/homeWidgets";
import queryKeys from "../queryKey";

/** How long a widget's view is fresh. Each read runs the app's function, so the
 *  home does not re-run every widget on every render or tab switch. */
export const WIDGETS_STALE_MS = 60_000;

/** The home's sections when the org has no layout, and while it loads. */
export const DEFAULT_SECTIONS: HomeSection[] = [
  { kind: "widgets", title: null, items: [] },
  { kind: "apps" },
  { kind: "insights" }
];

/** The soonest any widget asked to be read again (`refresh_after`), or no
 *  polling when none did. One request reads every widget, so the home re-reads
 *  them all on the shortest ask; the server already clamps each to 60 s+. */
export function refreshEvery(data: HomeWidgetsResponse | undefined): number | false {
  const asks = (data?.widgets ?? [])
    .map((w) => w.view?.refresh_after)
    .filter((s): s is number => typeof s === "number" && s > 0);
  return asks.length ? Math.min(...asks) * 1000 : false;
}

const homeWidgetsQuery = (orgSlug: string | undefined) => ({
  queryKey: queryKeys.homeWidgets.list(orgSlug ?? ""),
  queryFn: () => HomeWidgetsService.list(orgSlug ?? ""),
  enabled: !!orgSlug,
  staleTime: WIDGETS_STALE_MS,
  refetchInterval: (query: { state: { data?: HomeWidgetsResponse } }) =>
    refreshEvery(query.state.data)
});

/**
 * The widget copies the org's layout shows, of the apps the person can open in
 * `orgSlug`, each with its view or why there is none. Re-read on focus once
 * stale, on the soonest `refresh_after`, and after a button press.
 */
export const useHomeWidgets = (orgSlug: string | undefined) =>
  useQuery({ ...homeWidgetsQuery(orgSlug), select: (data: HomeWidgetsResponse) => data.widgets });

// Module scope, so React Query keeps the selected list while the data is the same.
const sectionsOf = (data: HomeWidgetsResponse) =>
  data.sections?.length ? data.sections : DEFAULT_SECTIONS;

/** The sections the org's layout draws, in order: the same request as the
 *  widgets, so it costs nothing more. The default while it loads or fails. */
export function useHomeSections(orgSlug: string | undefined): HomeSection[] {
  const { data } = useQuery({ ...homeWidgetsQuery(orgSlug), select: sectionsOf });
  return data ?? DEFAULT_SECTIONS;
}
