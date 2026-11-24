import {
  Activity,
  Database,
  KeyRound,
  LayoutDashboard,
  type LucideIcon,
  Radio,
  Rocket,
  Settings,
  SquareFunction
} from "lucide-react";

/**
 * The app console's pages, and the one rule for the URL each lives at.
 *
 * The console was one page of twenty open panels, ordered by six operator questions
 * and labelled with them ("Q1 — is it broken"). It answered everything in one scroll,
 * which meant every visit scrolled past everything. It is now a handful of short pages
 * by the job an operator came for, each with its own address, in the shape Encore's
 * console uses: a home that leads with numbers and what is broken, then one page per
 * area.
 *
 * A page lives at `/admin/apps/<org>/<app>/<segment>`; the overview is the bare app
 * route. Moving between pages keeps the environment switch (`?env=`) and drops state
 * that belongs to one page (`?fn=` is Functions'), so a link never carries a selection
 * into a page that cannot show it.
 */
export type ConsolePageId =
  | "overview"
  | "activity"
  | "functions"
  | "events"
  | "releases"
  | "data"
  | "secrets"
  | "settings";

export type ConsolePage = {
  id: ConsolePageId;
  label: string;
  /** The path segment after the app. Empty for the overview, which is the bare route. */
  segment: string;
  /**
   * The glyph that stands for the page.
   *
   * It lives on the page rather than in the component that draws it, because two
   * surfaces now draw this list — the console's own sidebar and the column browser's
   * Pages tier, which *is* that sidebar at `lg` and up. Two icon maps would be two
   * chances for the same page to look like two pages.
   */
  icon: LucideIcon;
};

export const CONSOLE_PAGES: readonly ConsolePage[] = [
  { id: "overview", label: "Overview", segment: "", icon: LayoutDashboard },
  { id: "activity", label: "Activity", segment: "activity", icon: Activity },
  { id: "functions", label: "Functions", segment: "functions", icon: SquareFunction },
  { id: "events", label: "Events", segment: "events", icon: Radio },
  { id: "releases", label: "Releases", segment: "releases", icon: Rocket },
  { id: "data", label: "Data", segment: "data", icon: Database },
  { id: "secrets", label: "Secrets", segment: "secrets", icon: KeyRound },
  { id: "settings", label: "Settings", segment: "settings", icon: Settings }
];

/**
 * The page a URL segment names: the overview when there is none, and `null` for a
 * segment no page has, so the caller can say so rather than silently showing the
 * overview under a URL that claims to be something else.
 */
export const consolePageFrom = (segment: string | undefined): ConsolePageId | null => {
  if (!segment) return "overview";
  return CONSOLE_PAGES.find((p) => p.segment === segment)?.id ?? null;
};

/** Query parameters that mean the same thing on every page, so a page change keeps them. */
const CARRIED = ["env"] as const;

/**
 * The address of `page` for one app, carrying only what every page reads from `search`,
 * plus `extra` (for a link that opens a page on a selection, such as one function).
 */
export const consolePagePath = (
  app: { org_slug: string; slug: string },
  page: ConsolePageId,
  search: string | URLSearchParams = "",
  extra: Record<string, string> = {}
): string => {
  const from = typeof search === "string" ? new URLSearchParams(search) : search;
  const next = new URLSearchParams();
  for (const key of CARRIED) {
    const value = from.get(key);
    if (value) next.set(key, value);
  }
  for (const [key, value] of Object.entries(extra)) next.set(key, value);
  const segment = CONSOLE_PAGES.find((p) => p.id === page)?.segment ?? "";
  const base = `/admin/apps/${encodeURIComponent(app.org_slug)}/${encodeURIComponent(app.slug)}`;
  const query = next.toString();
  return `${segment ? `${base}/${segment}` : base}${query ? `?${query}` : ""}`;
};
