// The in-app inbox, React-free: wire types, the four requests, and where a
// tapped notification goes. `useNotifications` (notifications-hooks.tsx) and
// `<NotificationsBell />` (notifications-bell.tsx) are thin over this.
//
// A custom app is its own page, never inside Oxygen's shell, so a person
// who lives in one app — crew on a kiosk, a single-app user — never sees
// Oxygen's bell. The app's host forwards `/api/*` to the product API with the
// app's own session, so the app reads the same inbox the product does
// (`crates/app/src/server/api/notifications/`). Design:
// internal-docs/custom-app-notifications.md.

import {
  GENERAL_CATEGORY,
  type NotificationAction,
  type NotificationCategoryRef,
  type NotificationLevel
} from "./notification-catalog";

export type { NotificationAction, NotificationCategoryRef, NotificationLevel };

/** The custom app a notification came from. */
export interface NotificationApp {
  id: string;
  slug: string;
  name: string;
}

/** One row of the inbox (`GET /api/notifications`), as the server sends it. */
export interface NotificationItem {
  id: string;
  org_id: string;
  kind: string;
  created_at: string;
  read_at: string | null;
  /** `null` when the org's store could not be read right now. */
  title: string | null;
  body: string | null;
  /**
   * Where Oxygen opens it: an app's lands in the app
   * (`/customer-apps/<org>/<app>/…`), a platform notice's is a product path.
   */
  href: string | null;
  /**
   * The path inside the app exactly as the app passed it to `ctx.notify`
   * (`/bookings/1`), for the app's own router. `null` when it passed none, and
   * for a platform notice. Absent (`undefined`) on a server older than the
   * field — which also cannot narrow the inbox to one app.
   */
  link?: string | null;
  /** `null` for a platform notice. */
  app: NotificationApp | null;
  // The fields below come from the app's notification catalogue (`oxy-app.json`
  // `notifications`). Absent on a server older than categories: read a missing
  // one as "nothing to add", never as an error.
  /** Where it sits in its source's tree; `general` for a kind with no category. */
  category?: NotificationCategoryRef;
  /** The kind's label (`"Temperature alarm"`). */
  kind_label?: string;
  level?: NotificationLevel;
  /**
   * Opaque: the hash of the `thread` the app passed to `ctx.notify`, `null`
   * when it passed none. Items sharing one collapse into the newest.
   */
  thread?: string | null;
  actions?: NotificationAction[];
  /** An absolute deep link, for sharing and phones. The bell follows `href` / `link`. */
  url?: string | null;
}

/** `GET /api/notifications`. */
export interface NotificationsPage {
  unread: number;
  items: NotificationItem[];
  /** The cursor for the next, older page; absent or `null` when there is none. */
  next_before?: string | null;
}

/**
 * What the list is narrowed to, beside the app scope. `category` is a path in
 * this app's catalogue and covers its subtree; `thread` is an item's `thread`.
 */
export interface NotificationsFilter {
  category?: string;
  kind?: string;
  thread?: string;
}

/** `"app"`: this app's notifications. `"all"`: the person's whole inbox. */
export type NotificationsScope = "app" | "all";

/**
 * What an inbox request came to. `unavailable`: this server or session has no
 * inbox. `status`: the HTTP status of a refusal (absent for a dropped
 * connection).
 */
export type InboxOutcome<T> =
  | { ok: true; value: T }
  | { ok: false; unavailable: boolean; error: Error; status?: number };

/**
 * The status a staging or `dev-*` app host answers both writes with
 * (`EnvironmentRefused`): read state carries no environment, so a staging
 * "mark all read" would clear production's unread.
 */
export const WRITE_REFUSED = 403;

type Fetcher = typeof fetch;

/**
 * Statuses that mean "there is no inbox here", not "the inbox failed": a
 * server that predates the routes (404/405), or nobody signed in who can read
 * one (401/403). A bell renders nothing for these instead of an error.
 */
const UNAVAILABLE = new Set([401, 403, 404, 405]);

async function errorOf(resp: Response): Promise<Error> {
  let detail = "";
  try {
    detail = (await resp.text()).slice(0, 240);
  } catch {
    // The status is enough.
  }
  return new Error(`notifications: HTTP ${resp.status}${detail ? ` — ${detail}` : ""}`);
}

async function request<T>(
  fetcher: Fetcher,
  url: string,
  init: RequestInit,
  parse: (resp: Response) => Promise<T>
): Promise<InboxOutcome<T>> {
  let resp: Response;
  try {
    resp = await fetcher(url, init);
  } catch (e) {
    return { ok: false, unavailable: false, error: e instanceof Error ? e : new Error(String(e)) };
  }
  if (!resp.ok) {
    return {
      ok: false,
      unavailable: UNAVAILABLE.has(resp.status),
      error: await errorOf(resp),
      status: resp.status
    };
  }
  try {
    return { ok: true, value: await parse(resp) };
  } catch {
    // A 200 that is not the inbox's JSON is a host answering every path with
    // a page (an app host does): there is no inbox behind it.
    return {
      ok: false,
      unavailable: true,
      error: new Error(`notifications: ${url} did not answer with the inbox`)
    };
  }
}

async function json<T>(resp: Response): Promise<T> {
  return (await resp.json()) as T;
}

function unreadOf(body: unknown): number {
  const n = (body as { unread?: unknown } | null)?.unread;
  if (typeof n !== "number" || !Number.isFinite(n)) throw new Error("no unread count");
  return n;
}

function pageOf(body: unknown): NotificationsPage {
  const items = (body as { items?: unknown } | null)?.items;
  if (!Array.isArray(items)) throw new Error("no items");
  const next = (body as { next_before?: unknown }).next_before;
  return {
    unread: unreadOf(body),
    items: items as NotificationItem[],
    ...(typeof next === "string" || next === null ? { next_before: next } : {})
  };
}

/**
 * A filter as the client sends it: `source` (an app id, or `"oxygen"`) is the
 * tree a `category` is a path in, which the server needs beside it.
 */
export interface InboxFilter extends NotificationsFilter {
  source?: string;
}

/** The four inbox requests, narrowed to one app when `appId` is set. */
export interface InboxClient {
  /** The badge: Oxy's own database only, so polling never wakes an org's store. */
  unread(filter?: InboxFilter): Promise<InboxOutcome<number>>;
  /** Newest first, read and unread, narrowed by `filter`. */
  list(limit: number, filter?: InboxFilter): Promise<InboxOutcome<NotificationsPage>>;
  markRead(id: string): Promise<InboxOutcome<void>>;
  /** Everything in scope read, or everything `filter` covers. */
  markAllRead(filter?: InboxFilter): Promise<InboxOutcome<void>>;
}

/** The filter as query parameters: only the keys that are set. */
function filterParams(filter: InboxFilter): Record<string, string> {
  const params: Record<string, string> = {};
  if (filter.source) params.source = filter.source;
  if (filter.category) params.category = filter.category;
  if (filter.kind) params.kind = filter.kind;
  if (filter.thread) params.thread = filter.thread;
  return params;
}

/** Whether `filter` narrows anything. */
export function isFiltered(filter: NotificationsFilter): boolean {
  return Boolean(filter.category || filter.kind || filter.thread);
}

/**
 * `apiBaseUrl` is the resolved manifest's (empty when oxy serves the bundle);
 * `fetcher` is the provider's, so `backendUrl` and credentials apply.
 */
export function createInboxClient(
  fetcher: Fetcher,
  apiBaseUrl: string,
  appId: string | undefined
): InboxClient {
  const root = `${apiBaseUrl.replace(/\/+$/, "")}/api/notifications`;
  const scoped = (path: string, extra: Record<string, string> = {}): string => {
    const params = new URLSearchParams(extra);
    if (appId) params.set("app_id", appId);
    const query = params.toString();
    return `${root}${path}${query ? `?${query}` : ""}`;
  };
  const get = { method: "GET" };
  // `keepalive`: tapping a notification marks it read and then usually leaves
  // the page (another app, or Oxygen, in this tab). Without it the browser may
  // cancel the request on the way out, and the item comes back unread.
  const post = { method: "POST", keepalive: true };
  return {
    unread: (filter = {}) =>
      request(fetcher, scoped("/unread", filterParams(filter)), get, async (r) =>
        unreadOf(await json(r))
      ),
    list: (limit, filter = {}) => {
      const query = { unread_only: "false", limit: String(limit), ...filterParams(filter) };
      return request(fetcher, scoped("", query), get, async (r) => pageOf(await json(r)));
    },
    markRead: (id) =>
      request(fetcher, `${root}/${encodeURIComponent(id)}/read`, post, async () => undefined),
    markAllRead: (filter = {}) =>
      request(fetcher, scoped("/read-all", filterParams(filter)), post, async () => undefined)
  };
}

/**
 * Did this page come from a server that knows categories, and so filters by
 * them (`category`, `kind`, `thread` — on the list, the count and "mark all
 * read")? An older one ignores the filter. `null` when there is nothing to
 * judge by.
 */
export function serverKnowsCategories(items: NotificationItem[]): boolean | null {
  if (items.length === 0) return null;
  return items.every((item) => item.category !== undefined);
}

/** An item's category path; `general` from a server older than categories. */
export function categoryPathOf(item: NotificationItem): string {
  return item.category?.path ?? GENERAL_CATEGORY;
}

/**
 * Whether `item` is inside `filter`: the server's rule, applied again so an
 * older server that ignored the filter cannot widen the list.
 */
export function matchesFilter(item: NotificationItem, filter: NotificationsFilter): boolean {
  if (filter.kind && item.kind !== filter.kind) return false;
  if (filter.thread && item.thread !== filter.thread) return false;
  if (filter.category) {
    const path = categoryPathOf(item);
    if (path !== filter.category && !path.startsWith(`${filter.category}/`)) return false;
  }
  return true;
}

/** One row of a list: the newest item of a thread run, and the older ones folded under it. */
export interface NotificationGroup {
  /** The newest — the one a row shows and a tap opens. */
  item: NotificationItem;
  /** The older items of the same thread right after it, newest first. */
  more: NotificationItem[];
}

/**
 * Fold each run of consecutive items sharing a `thread` into its newest, as a
 * phone stacks them. Items with no thread stand alone. Order is kept.
 */
export function collapseThreads(items: NotificationItem[]): NotificationGroup[] {
  const groups: NotificationGroup[] = [];
  for (const item of items) {
    const last = groups[groups.length - 1];
    if (last && item.thread && last.item.thread === item.thread) last.more.push(item);
    else groups.push({ item, more: [] });
  }
  return groups;
}

/** True while any item of the group is unread. */
export function groupUnread(group: NotificationGroup): boolean {
  return group.item.read_at === null || group.more.some((i) => i.read_at === null);
}

/**
 * Did this page come from a server that narrows the inbox to one app? The
 * `link` field shipped with the `app_id` filter, so an item without it is from
 * a server that ignored the filter — whose "mark all read" would clear every
 * app's notifications. `null` when there is nothing to judge by.
 */
export function serverScopesToApp(items: NotificationItem[]): boolean | null {
  if (items.length === 0) return null;
  return items.every((item) => item.link !== undefined);
}

// ── Where a tapped notification goes ────────────────────────────────────────

/** A path inside an app, and nothing that could leave it. */
function safeAppPath(path: string | null | undefined): string | null {
  if (!path?.startsWith("/") || path.startsWith("//") || path.includes("\\")) {
    return null;
  }
  return path;
}

const APP_HREF = /^\/customer-apps\/([^/?#]+)\/([^/?#]+)(?=[/?#]|$)(.*)$/;

/**
 * The app's base path in the address bar this page was loaded under: the main
 * host's `/customer-apps/<org>/<app>/`, an org subdomain's `/a/<app>/`, or `/`
 * on the app's own subdomain (and `pnpm dev` at the root).
 */
export function appBasePath(pathname: string, orgSlug: string, appSlug: string): string {
  const prefixes = [
    orgSlug && appSlug ? `/customer-apps/${orgSlug}/${appSlug}` : "",
    appSlug ? `/a/${appSlug}` : ""
  ];
  for (const prefix of prefixes) {
    if (prefix && (pathname === prefix || pathname.startsWith(`${prefix}/`))) return `${prefix}/`;
  }
  return "/";
}

/** `link` under `base`: `("/a/x/", "/bookings/1")` → `/a/x/bookings/1`. */
export function joinAppPath(base: string, link: string): string {
  return `${base.replace(/\/+$/, "")}${link}`;
}

/**
 * Leaving the page. A seam, not an option: jsdom cannot navigate, and its
 * `location` cannot be spied on, so the bell's tests replace these.
 */
export const pageNavigation = {
  assign: (url: string): void => window.location.assign(url),
  open: (url: string, target: string, features?: string): void => {
    window.open(url, target, features);
  }
};

/** Where tapping an item takes the person. */
export type NotificationTarget =
  /** This app: `link` is a path inside it, for its router. */
  | { kind: "app"; link: string }
  /**
   * Somewhere else: open `url` in the window named `window`. `_self` for
   * anywhere in Oxygen, another app included, which opens in this tab as the
   * dock does; null (a new tab) for a page outside Oxygen.
   */
  | { kind: "away"; url: string; window: string | null }
  /** Nothing to open; tapping only marks it read. */
  | { kind: "none" };

export interface TargetContext {
  /** This app's id (`window.__OXY_APP__.appId`); undefined in `pnpm dev`. */
  appId: string | undefined;
  /**
   * The Oxygen origin for a relative `href`, when this page is not served by
   * it (a standalone dev server). Unset: the `href` as the server sent it.
   */
  productBaseUrl?: string;
}

/** The path inside the app, from a server older than `link`: `href` minus the app's prefix. */
function linkFromHref(href: string | null): string | null {
  const rest = href ? APP_HREF.exec(href)?.[3] : undefined;
  if (!rest || rest === "/") return null;
  return safeAppPath(rest.startsWith("/") ? rest : `/${rest}`);
}

function awayTarget(href: string | null, productBaseUrl?: string): NotificationTarget {
  if (!href) return { kind: "none" };
  if (/^https?:\/\//i.test(href)) return { kind: "away", url: href, window: null };
  if (!safeAppPath(href)) return { kind: "none" };
  const url = productBaseUrl ? `${productBaseUrl.replace(/\/+$/, "")}${href}` : href;
  return { kind: "away", url, window: "_self" };
}

/**
 * This app's notification opens inside it, by its own `link`; anything else —
 * another app's, a platform notice — opens where Oxygen would open it.
 */
export function notificationTarget(item: NotificationItem, ctx: TargetContext): NotificationTarget {
  const own = ctx.appId !== undefined && item.app?.id === ctx.appId;
  if (!own) return awayTarget(item.href, ctx.productBaseUrl);
  const link = item.link === undefined ? linkFromHref(item.href) : safeAppPath(item.link);
  return link ? { kind: "app", link } : { kind: "none" };
}

// ── Display ─────────────────────────────────────────────────────────────────

/**
 * The category a row names: the leaf's label and the whole path's. `null` for
 * the implicit `general` category (it says nothing) and from an older server.
 */
export function categoryLabel(item: NotificationItem): { leaf: string; full: string } | null {
  const category = item.category;
  if (!category || category.path === GENERAL_CATEGORY) return null;
  const leaf = category.labels[category.labels.length - 1];
  return leaf ? { leaf, full: category.labels.join(" › ") } : null;
}

/** The badge text: nothing at zero, capped at "99+". */
export function badgeLabel(count: number): string | null {
  if (count <= 0) return null;
  return count > 99 ? "99+" : String(count);
}

/** "now", "5m", "3h", "2d", then a date. Empty for an unreadable timestamp. */
export function formatAge(iso: string, now: number = Date.now()): string {
  const at = Date.parse(iso);
  if (Number.isNaN(at)) return "";
  const seconds = Math.max(0, (now - at) / 1000);
  if (seconds < 60) return "now";
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m`;
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h`;
  if (seconds < 7 * 86400) return `${Math.floor(seconds / 86400)}d`;
  return new Date(at).toLocaleDateString(undefined, { month: "short", day: "numeric" });
}
