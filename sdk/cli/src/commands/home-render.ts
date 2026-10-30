/**
 * The home as plain text: what a person sees, in the order they see it, so an
 * agent can check a layout without a browser. `oxyc home show` and
 * `oxyc home apply --dry-run` share it.
 *
 * Plain on purpose: no colour, no table. The reader is mostly a model, and the
 * lines are cheap to read and to diff.
 */

import type { HomeCatalogEntry, HomeDescribedWidget, HomeDescription } from "@oxy-hq/sdk/manifest";
import type { FeedRow, FeedView, FeedWidget, WidgetsFeed } from "./home-client.js";

/** Rows drawn per widget; the card shows more, the text says how many it left out. */
const ROWS_SHOWN = 3;

const SECTION_TITLES: Record<string, string> = {
  widgets: "Widgets",
  apps: "Your apps",
  insights: "Insights"
};

/** Cards by `key` (`<app>/<widget>` or `<app>/<widget>#<id>`), from the widgets route. */
function viewsByKey(feed: WidgetsFeed | undefined): Map<string, FeedWidget> {
  const out = new Map<string, FeedWidget>();
  for (const w of feed?.widgets ?? []) out.set(w.key, w);
  return out;
}

/** `300` as `5m`, `5400` as `1h30m`, `90` as `1m30s`. */
function every(seconds: number): string {
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const sec = seconds % 60;
  return `${h ? `${h}h` : ""}${m ? `${m}m` : ""}${sec ? `${sec}s` : ""}`;
}

/** `store=mission, late_after=60`, or nothing for a widget with no settings. */
function paramsLine(params: HomeDescribedWidget["params"] | undefined): string | undefined {
  const entries = Object.entries(params ?? {});
  if (entries.length === 0) return undefined;
  return `params: ${entries.map(([k, v]) => `${k}=${v}`).join(", ")}`;
}

/** The settings a layout must give before the widget can show: required, with no default. */
function missingSettings(entry: HomeCatalogEntry): string[] {
  const props = entry.params?.properties ?? {};
  return (entry.params?.required ?? []).filter((name) => props[name]?.default === undefined);
}

function notShown(entry: HomeCatalogEntry): string {
  if (!entry.needs_params) return `${entry.widget} (${entry.title})`;
  const names = missingSettings(entry);
  const needs = names.length > 0 ? `needs settings: ${names.join(", ")}` : "needs settings";
  return `${entry.widget} (${entry.title}, ${needs})`;
}

function rowLine(row: FeedRow): string {
  const state = row.state ? `[${row.state}] ` : "";
  const detail = row.detail ? ` — ${row.detail}` : "";
  return `- ${state}${row.title ?? ""}${detail}`;
}

function metricLine(view: FeedView): string | undefined {
  const m = view.metric;
  if (!m?.value) return undefined;
  const parts = [m.value, m.label, m.delta, m.trend].filter((p): p is string => Boolean(p));
  return `metric ${parts.join(" · ")}`;
}

function viewLines(live: FeedWidget | undefined, previewing: boolean): string[] {
  if (!live) return [previewing ? "(no live view until it is saved)" : "(no view)"];
  if (live.error) return [`error: ${live.error}`];
  const view = live.view;
  if (!view) return ["(no view)"];
  const lines: string[] = [];
  if (view.headline) lines.push(`${view.headline} [${view.level ?? "info"}]`);
  if (view.detail) lines.push(view.detail);
  if (view.progress) lines.push(`progress ${view.progress.done}/${view.progress.total}`);
  const metric = metricLine(view);
  if (metric) lines.push(metric);
  const rows = view.rows ?? [];
  for (const row of rows.slice(0, ROWS_SHOWN)) lines.push(rowLine(row));
  if (rows.length > ROWS_SHOWN) lines.push(`… ${rows.length - ROWS_SHOWN} more`);
  const buttons = (view.buttons ?? []).map((b) => b.label).filter(Boolean);
  if (buttons.length > 0) lines.push(`buttons: ${buttons.join(", ")}`);
  return lines.length > 0 ? lines : ["(nothing to show)"];
}

/**
 * The home's own rule (`isCalm` in the web app): a view that is `ok` or `info`
 * and offers no button is folded to one line, after the cards with something
 * to do. A failed or missing view keeps its card.
 */
function isCalm(live: FeedWidget | undefined): boolean {
  const view = live?.view;
  if (!view || live?.error) return false;
  const level = view.level ?? "info";
  if (level !== "ok" && level !== "info") return false;
  return (view.buttons ?? []).length === 0 && (view.rows ?? []).every((row) => !row.button);
}

function widgetBlock(
  item: HomeDescribedWidget,
  views: Map<string, FeedWidget>,
  previewing: boolean
): string[] {
  const live = views.get(item.key);
  const calm = isCalm(live);
  const head = `${item.key} · ${item.title} · ${calm ? "compact" : item.size}`;
  const refresh = live?.view?.refresh_after;
  const facts = [
    paramsLine(item.params),
    refresh ? `refreshes every ${every(refresh)}` : undefined
  ].filter((l): l is string => Boolean(l));
  // A folded card shows its headline and nothing else.
  const view = calm ? viewLines(live, previewing).slice(0, 1) : viewLines(live, previewing);
  return [head, ...[...facts, ...view].map((l) => `  ${l}`)];
}

/** Cards with something to do first, calm ones after, each in the group's order: as the home draws them. */
function drawOrder(
  items: HomeDescribedWidget[],
  views: Map<string, FeedWidget>
): HomeDescribedWidget[] {
  const calm = (item: HomeDescribedWidget) => isCalm(views.get(item.key));
  return [...items.filter((i) => !calm(i)), ...items.filter(calm)];
}

/**
 * The description's sections in order, each widget with its live view.
 * `previewing` is a dry run: the feed reflects the saved layout, so a widget it
 * does not draw has no view to show yet.
 */
export function renderHome(
  home: HomeDescription,
  feed: WidgetsFeed | undefined,
  opts: { previewing?: boolean } = {}
): string {
  const views = viewsByKey(feed);
  const previewing = Boolean(opts.previewing);
  const lines: string[] = [];
  const state = home.customized ? `customized, revision ${home.revision}` : "default layout";
  lines.push(
    `${home.org.name} (${home.org.slug}) home — ${state}${previewing ? " — preview" : ""}`
  );
  for (const section of home.sections) {
    const heading = section.kind === "widgets" ? section.title : null;
    lines.push("", heading || (SECTION_TITLES[section.kind] ?? section.kind));
    if (section.kind !== "widgets") continue;
    if (section.items.length === 0) lines.push("  (no widgets)");
    drawOrder(section.items, views).forEach((item, i) => {
      if (i > 0) lines.push("");
      for (const l of widgetBlock(item, views, previewing)) lines.push(`  ${l}`);
    });
  }
  const hidden = home.catalog.filter((c) => !c.shown);
  if (hidden.length > 0) {
    lines.push("", `not shown: ${hidden.map(notShown).join(", ")}`);
  }
  return `${lines.join("\n")}\n`;
}
