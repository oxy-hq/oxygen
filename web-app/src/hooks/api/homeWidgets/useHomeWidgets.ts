import { useQuery } from "@tanstack/react-query";
import { HomeWidgetsService } from "@/services/api/homeWidgets";
import queryKeys from "../queryKey";

/** How long a widget's view is fresh. Each read runs the app's function, so the
 *  home does not re-run every widget on every render or tab switch. */
export const WIDGETS_STALE_MS = 60_000;

/**
 * Every widget of every app the person can open in `orgSlug`, each with its view
 * or why there is none. Re-read on focus once stale, and after a button press.
 */
export const useHomeWidgets = (orgSlug: string | undefined) =>
  useQuery({
    queryKey: queryKeys.homeWidgets.list(orgSlug ?? ""),
    queryFn: () => HomeWidgetsService.list(orgSlug ?? ""),
    select: (data) => data.widgets,
    enabled: !!orgSlug,
    staleTime: WIDGETS_STALE_MS
  });
