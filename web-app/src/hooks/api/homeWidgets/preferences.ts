import type {
  HiddenWidget,
  HomePreferences,
  HomeSection,
  HomeSectionKind,
  HomeWidgetsResponse,
  WidgetCard
} from "@/types/homeWidgets";

/** What a person who never chose anything has: nothing hidden, Today shown. */
export const DEFAULT_PREFERENCES: HomePreferences = { hidden: [], today: "shown" };

/** The choices alone: what a save sends, without the layout's `sections` the server adds. */
export function choicesOf(prefs: HomePreferences): HomePreferences {
  return { hidden: prefs.hidden, today: prefs.today };
}

/**
 * The sections the org layout draws, rebuilt from their kinds (a widgets group
 * names no copies here), for when the widgets are not read. `undefined` when the
 * server named none, so the caller falls back to its default.
 */
export function sectionsFromKinds(kinds: HomeSectionKind[] | undefined): HomeSection[] | undefined {
  if (!kinds?.length) return undefined;
  return kinds.map((kind) => (kind === "widgets" ? { kind, title: null, items: [] } : { kind }));
}

/** The preferences a widgets answer echoes, for when the preferences read has not landed. */
export function preferencesOf(data: HomeWidgetsResponse | undefined): HomePreferences | undefined {
  if (!data) return undefined;
  return { hidden: (data.hidden ?? []).map((h) => h.key), today: data.today ?? "shown" };
}

const named = (card: WidgetCard): HiddenWidget => ({
  key: card.key,
  title: card.title,
  app: card.app
});

/**
 * The widgets answer as the server will give it once `next` is stored: copies the
 * person hid leave `widgets` and their group's `items` and join `hidden`; ones they
 * restored leave `hidden` (their card returns when the widgets are read again, as
 * its function has to run). Pure, so the optimistic update and its test share it.
 */
export function applyPreferences(
  old: HomeWidgetsResponse,
  next: HomePreferences
): HomeWidgetsResponse {
  const hiding = new Set(next.hidden);
  const kept = (old.hidden ?? []).filter((h) => hiding.has(h.key));
  const keptKeys = new Set(kept.map((h) => h.key));
  const justHidden = old.widgets.filter((w) => hiding.has(w.key) && !keptKeys.has(w.key));
  return {
    ...old,
    widgets: old.widgets.filter((w) => !hiding.has(w.key)),
    sections: old.sections.map((s) =>
      s.kind === "widgets" ? { ...s, items: s.items.filter((key) => !hiding.has(key)) } : s
    ),
    hidden: [...kept, ...justHidden.map(named)],
    today: next.today
  };
}

/** Whether `next` brings back a copy that `before` hid. */
export function restores(before: HomePreferences, next: HomePreferences): boolean {
  const still = new Set(next.hidden);
  return before.hidden.some((key) => !still.has(key));
}
