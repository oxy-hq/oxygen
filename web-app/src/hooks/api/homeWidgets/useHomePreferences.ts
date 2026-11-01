import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { HomeWidgetsService } from "@/services/api/homeWidgets";
import type {
  HomePreferences,
  HomeWidgetsResponse,
  TodayPreference,
  WidgetCard
} from "@/types/homeWidgets";
import queryKeys from "../queryKey";
import {
  applyPreferences,
  choicesOf,
  DEFAULT_PREFERENCES,
  preferencesOf,
  restores
} from "./preferences";

/** A person's own choices change only here, so a read is good for a while. */
const PREFERENCES_STALE_MS = 5 * 60_000;

/**
 * The signed-in person's choices about their home in `orgSlug`: which widget
 * copies they hid, and whether Today shows. No retry: a failed read falls back
 * to the defaults rather than holding the home's Today back.
 */
export const useHomePreferences = (orgSlug: string | undefined) =>
  useQuery({
    queryKey: queryKeys.homePreferences.get(orgSlug ?? ""),
    queryFn: () => HomeWidgetsService.preferences(orgSlug ?? ""),
    enabled: !!orgSlug,
    staleTime: PREFERENCES_STALE_MS,
    retry: false
  });

/**
 * What the person has asked for while saves are in flight, per org. A save
 * writes the cache only after it has cancelled any read (an async step), so two
 * quick changes must not each build on a cache that has not caught up.
 */
const asked = new Map<string, HomePreferences>();

/**
 * Orgs where a save of the current burst of quick changes failed. A failure rolls
 * the caches back past any later optimistic change, and only the last save to
 * settle reads the server back, so that one must know an earlier one failed.
 */
const failedInBurst = new Set<string>();

type Snapshot = {
  preferences: HomePreferences | undefined;
  widgets: HomeWidgetsResponse | undefined;
  restored: boolean;
};

/**
 * Change what the home shows for this person: hide a copy, show it again, hide or
 * show Today. Optimistic, so the card leaves (and the line counts it) the moment it
 * is pressed, and rolled back with a toast if the server refuses — a card that
 * stayed hidden on screen but not on the server would come back on the next visit.
 *
 * Each action sends the whole set (the server stores it as one), built from the
 * latest cache, so two quick hides keep both.
 */
export function useHomePreferenceActions(orgSlug: string) {
  const qc = useQueryClient();
  const preferencesKey = queryKeys.homePreferences.get(orgSlug);
  const widgetsKey = queryKeys.homeWidgets.list(orgSlug);
  const mutationKey = [...preferencesKey, "save"] as const;

  const mutation = useMutation({
    mutationKey,
    mutationFn: (next: HomePreferences) => HomeWidgetsService.savePreferences(orgSlug, next),
    onMutate: async (next): Promise<Snapshot> => {
      await Promise.all([
        qc.cancelQueries({ queryKey: preferencesKey }),
        qc.cancelQueries({ queryKey: widgetsKey })
      ]);
      const preferences = qc.getQueryData<HomePreferences>(preferencesKey);
      const widgets = qc.getQueryData<HomeWidgetsResponse>(widgetsKey);
      const before = preferences ?? preferencesOf(widgets) ?? DEFAULT_PREFERENCES;
      // The layout's sections come from the server and are not the person's to change.
      qc.setQueryData<HomePreferences>(
        preferencesKey,
        preferences?.sections ? { ...next, sections: preferences.sections } : next
      );
      qc.setQueryData<HomeWidgetsResponse>(widgetsKey, (old) =>
        old ? applyPreferences(old, next) : old
      );
      return { preferences, widgets, restored: restores(before, next) };
    },
    onError: (_error, _next, snapshot) => {
      failedInBurst.add(orgSlug);
      qc.setQueryData(preferencesKey, snapshot?.preferences);
      qc.setQueryData(widgetsKey, snapshot?.widgets);
      toast.error("Couldn't save that. Try again.");
    },
    onSettled: (_data, _error, _next, snapshot) => {
      // Only the last of several quick changes reads the server back (this one still counts).
      const last = qc.isMutating({ mutationKey }) === 1;
      let rolledBack = false;
      if (last) {
        asked.delete(orgSlug);
        rolledBack = failedInBurst.delete(orgSlug);
        qc.invalidateQueries({ queryKey: preferencesKey });
      }
      // A copy brought back has to run its function to draw; a refused change (this one
      // or an earlier one of the burst) may have rolled back past a later one, so the
      // widgets are read again.
      if (snapshot?.restored || rolledBack) qc.invalidateQueries({ queryKey: widgetsKey });
    }
  });

  const current = (): HomePreferences =>
    asked.get(orgSlug) ??
    qc.getQueryData<HomePreferences>(preferencesKey) ??
    preferencesOf(qc.getQueryData<HomeWidgetsResponse>(widgetsKey)) ??
    DEFAULT_PREFERENCES;
  const save = (change: (was: HomePreferences) => HomePreferences) => {
    // The PUT body is the choices alone, whatever the cache also holds.
    const next = choicesOf(change(current()));
    asked.set(orgSlug, next);
    mutation.mutate(next);
  };

  return {
    hide: (card: Pick<WidgetCard, "key">) =>
      save((was) => ({ ...was, hidden: [...was.hidden.filter((k) => k !== card.key), card.key] })),
    show: (key: string) => save((was) => ({ ...was, hidden: was.hidden.filter((k) => k !== key) })),
    setToday: (today: TodayPreference) => save((was) => ({ ...was, today }))
  };
}
