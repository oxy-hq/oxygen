// A section's screens as data, shared by ParentTabs and its top-bar placement (TopBarTabs):
// the tab's shape, which tab is open, and how a link click reaches the app's router.

import type { MouseEvent, ReactNode } from "react";

/** One tab. Keyed by a plain string, the section's own union, so a typo in `id` or `also` is a
 *  compile error rather than a tab that never lights. */
export interface ParentTab<K extends string> {
  /** The value of `current` that lights this tab. */
  id: K;
  /** Further values that also count as this tab being open: a detail view that is a MODE of
   *  this tab rather than another place (a store's own page keeps Stores lit). */
  also?: readonly K[];
  label: string;
  /** The screen's URL, so ⌘-click and a copied link open it. */
  href: string;
  /** The screen's glyph before the label, `aria-hidden`: the label names it. */
  icon?: ReactNode;
  /** A count or mark after the label (`<span className={ui.tabCount}>`). */
  trailing?: ReactNode;
  /** The same count as a number, for the More button when this tab is in its menu, and the
   *  words that say what it counts ("3 to tag"). */
  count?: number;
  countLabel?: string;
}

/** Where a strip is drawn. `topbar` and `page` are the two halves of a section's strip and are
 *  never visible together; `inline` shows at every width (a gallery specimen, a page with no
 *  shell). */
export type ParentTabsPlacement = "topbar" | "page" | "inline";

/** The width from which the strip rides in the top bar: 768px (`md`, where OxyShell swaps its
 *  menu button for the rail) or 1280px (`xl`, a top bar already carrying something else). */
export type TopBarFrom = "md" | "xl";

export interface ParentTabsProps<K extends string> {
  /** Names the CHOICE, not the section: the h1 below already says which screen this is. */
  label: string;
  tabs: readonly ParentTab<K>[];
  current: K;
  /** A plain left click on a tab, for the app's router. Absent: the links navigate natively. */
  onNavigate?: (id: K, tab: ParentTab<K>) => void;
  placement?: ParentTabsPlacement;
  topBarFrom?: TopBarFrom;
  /** The More button's name in the top bar (default "More screens"). */
  moreLabel?: string;
  className?: string;
}

/** A plain left click is the app's own navigation; anything else is the browser's. */
export function isPlainClick(e: MouseEvent): boolean {
  return !(
    e.defaultPrevented ||
    e.metaKey ||
    e.ctrlKey ||
    e.shiftKey ||
    e.altKey ||
    e.button !== 0
  );
}

/** Whether `tab` is the open screen. */
export function isOpen<K extends string>(tab: ParentTab<K>, current: K): boolean {
  return tab.id === current || (tab.also?.includes(current) ?? false);
}

/** The click handler a tab link takes: the app's router on a plain left click. */
export function linkClick<K extends string>(
  tab: ParentTab<K>,
  onNavigate: ParentTabsProps<K>["onNavigate"],
  after?: () => void
) {
  return (e: MouseEvent<HTMLAnchorElement>) => {
    after?.();
    if (!onNavigate || !isPlainClick(e)) return;
    e.preventDefault();
    onNavigate(tab.id, tab);
  };
}
