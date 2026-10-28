import type { InboxItem, NotificationLevel, SourceNode } from "@/types/notifications";

const APP_PATH = /^\/customer-apps\/[^/?#]+\/[^/?#]+(?:[/?#]|$)/;

/**
 * Does a notification's link go into a custom app? An app is a document of its
 * own, outside this SPA's router, so it needs a real navigation rather than a
 * router `Link`. It opens in this tab, as every other way into an app does.
 */
export function isAppHref(href: string): boolean {
  return APP_PATH.test(href);
}

/** The implicit category of a kind that names none. Not worth a label on a row. */
export const GENERAL_CATEGORY = "general";

/**
 * Where a notification came from, for its row: "Store Ops · Food safety". Just
 * the source for the implicit General category, or from a server too old to
 * send a category.
 */
export function sourceLine(item: InboxItem): string {
  const source = item.app?.name ?? "Oxygen";
  const category = item.category;
  if (!category || category.path === GENERAL_CATEGORY) return source;
  const leaf = category.labels.at(-1);
  return leaf ? `${source} · ${leaf}` : source;
}

/** The levels that earn a row an accent, and what a screen reader hears for it. */
export const LEVEL_ACCENT: Partial<
  Record<NotificationLevel, { border: string; text: string; label: string }>
> = {
  critical: { border: "border-l-destructive", text: "text-destructive", label: "Critical" },
  time_sensitive: { border: "border-l-warning", text: "text-warning", label: "Time sensitive" }
};

/** Oxygen above the apps, the apps in the order the server sent them. */
export function oxygenFirst<T extends Pick<SourceNode, "source">>(sources: T[]): T[] {
  return [
    ...sources.filter((s) => s.source === "oxygen"),
    ...sources.filter((s) => s.source !== "oxygen")
  ];
}
