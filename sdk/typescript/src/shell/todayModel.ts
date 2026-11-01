// What the Today panel draws, read out of `GET /api/home/widgets?org=<slug>`.
//
// The payload is a server's answer and the SDK runs inside customers' apps, so
// nothing here trusts it: every field is checked as it is read, an entry that
// does not parse is skipped, and a payload that is not a widget list at all
// reads as `null` (the panel says it couldn't load). Nothing throws.

/** Where a row sits: cards that want a look first, then calm ones, then failures. */
export type TodayGroup = "attention" | "calm" | "failed";

export interface TodayRow {
  key: string;
  title: string;
  headline: string;
  detail: string | null;
  /** Where the row goes, already checked (http(s) or app-relative); null: not a link. */
  href: string | null;
  iconUrl: string | null;
  /** Initials for when the app has no icon. */
  initials: string;
  /** 0..1 when the view reports progress with a positive total. */
  progress: { done: number; total: number; fraction: number } | null;
  group: TodayGroup;
}

const GROUP_ORDER: Record<TodayGroup, number> = { attention: 0, calm: 1, failed: 2 };

type Json = Record<string, unknown>;

function isObject(value: unknown): value is Json {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function text(value: unknown): string | null {
  if (typeof value !== "string") return null;
  const trimmed = value.trim();
  return trimmed ? trimmed : null;
}

/** A link that is safe to put in an `href`: http(s), or a path on this host.
 *  `javascript:`, `data:` and protocol-relative `//host` URLs never pass. */
export function safeUrl(value: unknown): string | null {
  const url = text(value);
  if (!url) return null;
  if (/^https?:\/\//i.test(url)) return url;
  if (url.startsWith("/") && !url.startsWith("//") && !url.startsWith("/\\")) return url;
  return null;
}

function initialsOf(name: string): string {
  const letters = name
    .split(/\s+/)
    .filter(Boolean)
    .slice(0, 2)
    .map((word) => Array.from(word)[0] ?? "");
  return letters.join("").toUpperCase() || "?";
}

function progressOf(value: unknown): TodayRow["progress"] {
  if (!isObject(value)) return null;
  const { done, total } = value;
  if (typeof done !== "number" || typeof total !== "number") return null;
  if (!Number.isFinite(done) || !Number.isFinite(total) || total <= 0) return null;
  const clamped = Math.min(Math.max(done, 0), total);
  return { done: clamped, total, fraction: clamped / total };
}

function wantsAttention(view: Json): boolean {
  const level = text(view.level);
  if (level === "attention" || level === "critical" || level === "warning") return true;
  // A card with a button is asking for something, whatever its level says.
  const buttons = Array.isArray(view.buttons) ? view.buttons : view.actions;
  return Array.isArray(buttons) && buttons.length > 0;
}

function rowOf(card: unknown, index: number): TodayRow | null {
  if (!isObject(card)) return null;
  const app = isObject(card.app) ? card.app : {};
  const appName = text(app.name);
  const title = text(card.title) ?? appName;
  if (!title) return null;
  const view = isObject(card.view) ? card.view : null;
  const headline = view ? text(view.headline) : null;
  // No usable view is a failed card, whether the server said why or not.
  const failed = !view || !headline || text(card.error) !== null;
  const group: TodayGroup = failed ? "failed" : wantsAttention(view) ? "attention" : "calm";
  return {
    key: text(card.key) ?? `${title}:${index}`,
    title,
    headline: failed ? "Couldn't load" : (headline ?? ""),
    detail: failed || !view ? null : text(view.detail),
    href: (view && (safeUrl(view.href) ?? safeUrl(view.link))) || safeUrl(app.url),
    iconUrl: safeUrl(app.icon_url),
    initials: initialsOf(appName ?? title),
    progress: failed || !view ? null : progressOf(view.progress),
    group
  };
}

/** The rows of a widgets payload in display order, or `null` when it is not one. */
export function todayRows(payload: unknown): TodayRow[] | null {
  if (!isObject(payload) || !Array.isArray(payload.widgets)) return null;
  const rows: TodayRow[] = [];
  payload.widgets.forEach((card, index) => {
    const row = rowOf(card, index);
    if (row) rows.push(row);
  });
  // Stable: the server's order holds within each group.
  return rows
    .map((row, index) => ({ row, index }))
    .sort((a, b) => GROUP_ORDER[a.row.group] - GROUP_ORDER[b.row.group] || a.index - b.index)
    .map(({ row }) => row);
}
