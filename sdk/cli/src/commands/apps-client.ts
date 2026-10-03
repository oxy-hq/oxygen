/**
 * The wire half of `oxyc apps`: one GET helper, the walk of the paged app
 * listing, and resolution of `<org>/<app>` to its listing ROW.
 *
 * `../apps/resolve.ts` resolves an app to its id and slugs, which is all
 * `checks` and the sandbox commands need. The commands here print what the
 * listing says about an app (published, last active, source), so they resolve
 * to the whole row instead — and share that module's credential rule,
 * `ensureOk` and `UUID_RE` rather than keeping copies.
 *
 * The app listing (`GET /api/customer-apps`) pages by `limit`/`offset` and
 * answers `{items, next_offset}`; `next_offset: null` is the last page.
 */

import { parseJson, request } from "../api/request.js";
import { type Creds, ensureOk, staffCreds, UUID_RE } from "../apps/resolve.js";
import type { Context } from "../context/resolve.js";
import { CliError, ExitCode, usageError } from "../util/errors.js";

/** What a request needs besides its path. */
export type Conn = Creds;

/** The fields of an app row that name it. Every listing row carries them. */
export interface AppIdentity {
  id: string;
  slug: string;
  org_slug: string;
}

interface AppsPage<Row> {
  items?: Row[];
  next_offset?: number | null;
}

/** The app-admin surface: the same handlers as `/api/admin/apps`, gated by the app-admin role. */
export const CUSTOMER_APPS = "/api/customer-apps";

const APP_PAGE_SIZE = 100;
/** Hard stop on the listing walk, mirroring `api/paginate.ts`'s `MAX_PAGES`. */
const MAX_APP_PAGES = 100;

/**
 * GET `path` and return its JSON body, or throw the error its status deserves.
 *
 * A 2xx that is not JSON throws too. A deployment that answers an unknown path
 * with its HTML page and a 200 would otherwise read as an empty result.
 */
export async function getJson<T>(conn: Conn, path: string): Promise<T> {
  const response = await request({
    target: conn.target,
    path,
    method: "GET",
    bearer: conn.bearer,
    headers: conn.headers
  });
  ensureOk(response);
  const parsed = parseJson(response.body);
  if (parsed === undefined || parsed === null) {
    throw new CliError(`GET ${path} did not return JSON`, {
      code: ExitCode.FAILURE,
      detail: response.body.trim().slice(0, 500) || undefined,
      hint: "the deployment may predate this route — `oxyc routes customer-apps` lists what it mounts"
    });
  }
  return parsed as T;
}

/**
 * Walk the app listing at `listPath`, one page at a time.
 *
 * `visit` sees each page's rows and returns `true` to stop early. The result
 * says whether the walk reached the last page (or was stopped by `visit`):
 * `complete: false` means the page cap was hit with rows left unread.
 */
export async function walkApps<Row extends AppIdentity>(
  conn: Conn,
  listPath: string,
  visit: (rows: Row[]) => boolean
): Promise<{ complete: boolean }> {
  let offset = 0;
  for (let page = 0; page < MAX_APP_PAGES; page++) {
    const path = `${listPath}?limit=${APP_PAGE_SIZE}&offset=${offset}`;
    const payload = await getJson<AppsPage<Row>>(conn, path);
    if (!Array.isArray(payload.items)) {
      throw new CliError(`GET ${path} did not return an app listing`, { code: ExitCode.FAILURE });
    }
    if (visit(payload.items)) return { complete: true };
    if (payload.next_offset == null) return { complete: true };
    // An offset that does not advance would repeat this page until the cap.
    if (payload.next_offset <= offset) {
      throw new CliError(`GET ${path} answered next_offset ${payload.next_offset}`, {
        code: ExitCode.FAILURE,
        detail: "the next offset is not past the current one, so the listing cannot be walked"
      });
    }
    offset = payload.next_offset;
  }
  return { complete: false };
}

/** Every row of the listing, in the server's order, each app once. */
export async function listAllApps<Row extends AppIdentity>(
  conn: Conn,
  listPath: string
): Promise<{ rows: Row[]; complete: boolean }> {
  // The listing is ordered by `updated_at`, which can change between two page
  // requests and move a row across the page boundary — hence the id check.
  const seen = new Set<string>();
  const rows: Row[] = [];
  const { complete } = await walkApps<Row>(conn, listPath, (page) => {
    for (const row of page) {
      if (seen.has(row.id)) continue;
      seen.add(row.id);
      rows.push(row);
    }
    return false;
  });
  return { rows, complete };
}

/** `"<org-slug>/<app-slug>"` split at the first slash. */
export function splitAppRef(app: string): { orgSlug: string; appSlug: string } {
  const [orgSlug = "", ...rest] = app.split("/");
  return { orgSlug, appSlug: rest.join("/") };
}

/** Refuse an `<app>` argument that is neither `<org>/<app>` nor a UUID, before any request. */
export function requireAppRef(app: string): void {
  if (UUID_RE.test(app)) return;
  const { orgSlug, appSlug } = splitAppRef(app);
  if (!orgSlug || !appSlug) {
    throw usageError(`"${app}" does not name an app`, APP_REF_HINT);
  }
}

const APP_REF_HINT = '<app> is "<org-slug>/<app-slug>" or an app UUID';

/**
 * One row of the app listing — `admin::apps::dto::AppResponse`.
 *
 * The optional fields are the ones the server omits rather than sending
 * `null` or `false`: an app nobody has opened has no `last_active_at`, and
 * `source_unrecorded` is present only when it is `true`.
 */
export interface AppRow extends AppIdentity {
  name: string;
  org_id: string;
  project_id: string;
  branch: string;
  /** The repository the app is REGISTERED against — not where a build came from. */
  source_repo: string;
  status: string;
  url: string;
  url_subdomain?: string | null;
  repo_path?: string | null;
  /** `null` is a draft: nothing is served to the customer. */
  published_at?: string | null;
  last_active_at?: string;
  last_promoted_by_email?: string;
  last_promoted_at?: string;
  live_published_via?: string;
  /** The build being served records no repository or no commit. */
  source_unrecorded?: boolean;
  created_at: string;
  updated_at: string;
}

/** One row of `GET …/{id}/builds` — `admin::apps::dto::BuildSummary`. */
export interface BuildSummary {
  id: string;
  build_id: string;
  created_at: string;
  /** This is the build the draft channel points at. */
  is_draft: boolean;
  /** This is the build being served. One build can be both. */
  is_published: boolean;
  published_by_email: string | null;
  /** The workflow identity of a build published by CI through OIDC. */
  published_via: string | null;
  /** The raw git remote URL `oxyc publish` recorded, not an `<owner>/<name>` slug. */
  source_repo: string | null;
  commit_sha: string | null;
  source_branch: string | null;
}

export interface BuildHistory {
  builds: BuildSummary[];
  promoted_at: string | null;
  promoted_by_email: string | null;
}

/**
 * The target and a user credential for it, or the `authError` every command
 * throws. `staffCreds` because these are staff routes: it takes the bearer or
 * the API key, and refuses a publish token, which may not list apps.
 */
export function connect(ctx: Context): Conn {
  return staffCreds(ctx);
}

/**
 * The listing row for `<org-slug>/<app-slug>` or an app UUID.
 *
 * A UUID is matched against the listing too rather than fetched with
 * `GET …/{id}`: that single-app response leaves out the fields the listing
 * fills with batched queries (`last_active_at`, `source_unrecorded`,
 * `live_published_via`), and a row that silently lacks them would print as
 * "never active".
 */
async function resolveAppRow(conn: Conn, app: string): Promise<AppRow> {
  const { orgSlug, appSlug } = splitAppRef(app);
  const wanted = UUID_RE.test(app)
    ? (item: AppRow) => item.id.toLowerCase() === app.toLowerCase()
    : (item: AppRow) => item.org_slug === orgSlug && item.slug === appSlug;
  const found: { row?: AppRow } = {};
  await walkApps<AppRow>(conn, CUSTOMER_APPS, (rows) => {
    found.row = rows.find(wanted);
    return found.row !== undefined;
  });
  if (found.row) return found.row;
  throw new CliError(`no app "${app}"`, { code: ExitCode.NOT_FOUND, hint: APP_REF_HINT });
}

/**
 * Check `<app>`, then connect, then resolve — in that order.
 *
 * A malformed argument is a usage error whether or not a credential resolves,
 * so it is refused before the bearer is asked for and before any request.
 */
export async function openApp(ctx: Context, app: string): Promise<{ conn: Conn; row: AppRow }> {
  requireAppRef(app);
  const conn = connect(ctx);
  return { conn, row: await resolveAppRow(conn, app) };
}

export function appLabel(app: AppIdentity): string {
  return `${app.org_slug}/${app.slug}`;
}

/** An app's build history, newest first. */
export async function fetchBuilds(conn: Conn, appId: string): Promise<BuildHistory> {
  const path = `${CUSTOMER_APPS}/${appId}/builds`;
  const history = await getJson<BuildHistory>(conn, path);
  if (!Array.isArray(history.builds)) {
    throw new CliError(`GET ${path} did not return a build history`, { code: ExitCode.FAILURE });
  }
  const builds = [...history.builds].sort(
    (a, b) => Date.parse(b.created_at) - Date.parse(a.created_at)
  );
  return { ...history, builds };
}

/** The build being served, if any. */
export function liveBuild(history: BuildHistory): BuildSummary | undefined {
  return history.builds.find((build) => build.is_published);
}

/**
 * The draft build, when it is a different and newer build than the live one.
 *
 * The draft channel points at the live build itself right after a promote, and
 * at an older build after a rollback; neither is something waiting to ship.
 */
export function draftAheadOfLive(history: BuildHistory): BuildSummary | undefined {
  const draft = history.builds.find((build) => build.is_draft);
  if (!draft) return undefined;
  const live = liveBuild(history);
  if (!live) return draft;
  if (draft.id === live.id) return undefined;
  return Date.parse(draft.created_at) > Date.parse(live.created_at) ? draft : undefined;
}

/**
 * Run `task` over `items` with at most `limit` in flight, keeping input order.
 *
 * For the one-request-per-app fan-outs: unbounded, a listing of a few hundred
 * apps would open a few hundred connections to one deployment at once.
 */
export async function mapBounded<T, R>(
  items: T[],
  limit: number,
  task: (item: T) => Promise<R>
): Promise<R[]> {
  const results = new Array<R>(items.length);
  let next = 0;
  const worker = async (): Promise<void> => {
    for (;;) {
      const index = next++;
      if (index >= items.length) return;
      results[index] = await task(items[index] as T);
    }
  };
  await Promise.all(Array.from({ length: Math.min(limit, items.length) }, worker));
  return results;
}
