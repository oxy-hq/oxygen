import { useEffect, useMemo, useRef, useState } from "react";
import type { WidgetCard } from "@/types/homeWidgets";

/** What a widget showed when this browser last left the home: its key, then a
 *  fingerprint of its view. */
type Seen = Record<string, string>;

/** How a widget differs from the last visit: one the home did not show then, or one
 *  whose view has changed since. */
export type Since = "new" | "updated";

const NOTHING = new Map<string, Since>();

const key = (orgSlug: string) => `oxy.home.seen.${orgSlug}`;

function read(orgSlug: string): Seen | null {
  try {
    const raw = window.localStorage.getItem(key(orgSlug));
    const seen: unknown = raw ? JSON.parse(raw) : null;
    return seen && typeof seen === "object" ? (seen as Seen) : null;
  } catch {
    return null;
  }
}

/** What the person saw, folded into what they had seen before: a widget that
 *  failed this visit keeps its last view, so it does not come back as new. */
function remember(orgSlug: string, widgets: WidgetCard[]) {
  const seen: Seen = { ...read(orgSlug) };
  for (const card of widgets) {
    if (card.view) seen[card.key] = widgetFingerprint(card);
  }
  try {
    window.localStorage.setItem(key(orgSlug), JSON.stringify(seen));
  } catch {
    // Private mode or full storage: the next visit marks nothing.
  }
}

/** A short, stable digest of what a widget says: what a person would notice changing. */
export function widgetFingerprint(card: WidgetCard): string {
  const view = card.view;
  if (!view) return "";
  const said = JSON.stringify([
    view.level,
    view.headline,
    view.detail ?? null,
    view.badge ?? null,
    view.metric?.value ?? null,
    view.progress ? [view.progress.done, view.progress.total] : null,
    view.rows.map((row) => [row.title, row.state ?? null])
  ]);
  let hash = 5381;
  for (let i = 0; i < said.length; i++) hash = ((hash << 5) + hash + said.charCodeAt(i)) >>> 0;
  return hash.toString(36);
}

/**
 * Which widgets have news since the person last left this org's home in this
 * browser: the way a messaging app marks what arrived while you were away.
 *
 * The last visit is remembered when the person leaves (the tab is hidden, the page
 * goes, or the home unmounts), not as they look, so the marks hold for the whole
 * visit, and a quick look at another tab does not clear them. Coming Back from an
 * app restores the page as it was (the back-forward cache), so the home re-reads
 * what it remembered on the way out: the marks the person already saw are gone,
 * and only what changed while they were in the app is marked.
 *
 * A first visit marks nothing: with no memory there is nothing to compare.
 */
export function useSinceLastVisit(
  orgSlug: string | undefined,
  widgets: WidgetCard[] | undefined
): ReadonlyMap<string, Since> {
  // Each memory belongs to its org: switching org keeps the home mounted, and
  // neither what is compared against nor what is remembered may cross over.
  const [baseline, setBaseline] = useState<{ orgSlug: string; seen: Seen | null } | null>(() =>
    orgSlug ? { orgSlug, seen: read(orgSlug) } : null
  );
  // What each org's home last showed, so leaving org A remembers A's widgets even
  // when the render that switched to B has already happened.
  const shown = useRef(new Map<string, WidgetCard[]>());
  if (orgSlug && widgets) shown.current.set(orgSlug, widgets);

  useEffect(() => {
    if (!orgSlug) return;
    const reread = () => setBaseline({ orgSlug, seen: read(orgSlug) });
    reread();
    const leave = () => {
      const last = shown.current.get(orgSlug);
      if (last) remember(orgSlug, last);
    };
    const onVisibility = () => {
      if (document.visibilityState === "hidden") leave();
    };
    const onPageShow = (event: PageTransitionEvent) => {
      if (event.persisted) reread();
    };
    document.addEventListener("visibilitychange", onVisibility);
    window.addEventListener("pagehide", leave);
    window.addEventListener("pageshow", onPageShow);
    return () => {
      leave();
      document.removeEventListener("visibilitychange", onVisibility);
      window.removeEventListener("pagehide", leave);
      window.removeEventListener("pageshow", onPageShow);
    };
  }, [orgSlug]);

  return useMemo(() => {
    // Another org's memory, for the one render before the switch re-reads it.
    if (!baseline?.seen || baseline.orgSlug !== orgSlug || !widgets) return NOTHING;
    const seen = baseline.seen;
    const since = new Map<string, Since>();
    for (const card of widgets) {
      if (!card.view) continue;
      const before = seen[card.key];
      if (before === undefined) since.set(card.key, "new");
      else if (before !== widgetFingerprint(card)) since.set(card.key, "updated");
    }
    return since;
  }, [baseline, orgSlug, widgets]);
}
