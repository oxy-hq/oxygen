/**
 * The app resolution and credential helpers every sandbox-aware command
 * shares: `resolveApp` and `ensureOk` moved here from `commands/checks.ts`
 * (which still imports them), plus `staffCreds` — new for the sandbox
 * management, verify-and-read-back and logs routes, which are a staff
 * console surface and refuse a publish token outright (`custom-app-sandboxes.md`
 * §1 "Who may use one"; D22).
 *
 * `checks.ts` keeps its OWN credential resolution: a publish token (minted
 * here or exchanged from GitHub OIDC) is how CI runs `checks run` today, and
 * that must keep working. `staffCreds` is for the verbs that have no such
 * machine caller — a publish token is refused on every sandbox operation,
 * full stop.
 */

import { type ApiResponse, errorForResponse, parseJson, request } from "../api/request.js";
import { isMachineIdentity } from "../auth/token-kind.js";
import type { Context } from "../context/resolve.js";
import { authError, CliError, ExitCode, usageError } from "../util/errors.js";

export interface Creds {
  target: string;
  bearer?: string;
  headers?: Record<string, string>;
}

export interface ResolvedApp {
  appId: string;
  label: string;
  orgSlug: string;
  appSlug: string;
}

/** The prefix `oxyc publish` mints and the GitHub OIDC exchange returns. */
export const PUBLISH_TOKEN_PREFIX = "oxypublish_";
/** An app id, as opposed to an `<org-slug>/<app-slug>` pair. */
export const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
/** Hard stop on the admin-apps walk, mirroring `api/paginate.ts`'s `MAX_PAGES`. */
const MAX_APP_PAGES = 100;

/**
 * A user credential: the stored bearer, else the API key. Refuses an
 * `oxypublish_` bearer before anything else — every route this backs is a
 * staff console surface a publish token may never reach (D22).
 */
export function staffCreds(ctx: Context): Creds {
  // The STORED bearer, never an OIDC exchange: a minted token is a service
  // account's, which holds no staff standing and is refused on every route here.
  const bearer = ctx.storedBearer();
  if (bearer) {
    if (bearer.startsWith(PUBLISH_TOKEN_PREFIX)) {
      throw usageError(
        "a publish token cannot use this command",
        // Worded for every caller: sandbox management and `oxyc apps` both come here.
        "it needs a staff credential — `oxyc login`, or OXY_TOKEN set to a user token"
      );
    }
    // A service account's token is the other machine credential: no staff standing.
    if (isMachineIdentity(bearer)) {
      throw usageError(
        "a service-account token cannot use this command",
        "it needs a staff credential — `oxyc login`, or OXY_TOKEN set to a user token"
      );
    }
    return { target: ctx.target(), bearer };
  }
  const apiKey = ctx.apiKey();
  if (apiKey) return { target: ctx.target(), headers: { "X-API-Key": apiKey } };
  // Nothing resolved — the canonical authError naming the login command.
  throw authError(ctx.target(), ctx.flags.env ?? "production", ctx.flags.tokenEnv ?? "OXY_TOKEN");
}

export function ensureOk(response: ApiResponse): void {
  if (response.status < 200 || response.status >= 300) throw errorForResponse(response);
}

interface AdminAppsPage {
  items?: { id: string; slug: string; org_slug: string }[];
  next_offset?: number | null;
}

interface AdminAppDetail {
  id: string;
  slug: string;
  org_slug: string;
}

/**
 * Resolve `<org-slug>/<app-slug>` or an app UUID to its id and slugs.
 *
 * The org/app form is resolved by paging `GET /api/admin/apps` — the matched
 * row already carries `org_slug` and `slug`, so no extra request. A UUID
 * needs one more call, `GET /api/admin/apps/{id}`, to learn them — the shape
 * `fn call` and `logs` need for their `/customer-apps/{org}/{app}/...`
 * routes. Both paths are the admin surface: a publish token never reaches
 * this function (`checks.ts` short-circuits around it for that credential;
 * every other caller goes through `staffCreds`, which already refused one).
 */
export async function resolveApp(creds: Creds, app: string): Promise<ResolvedApp> {
  if (UUID_RE.test(app)) {
    const response = await request({
      target: creds.target,
      path: `/api/admin/apps/${app}`,
      method: "GET",
      bearer: creds.bearer,
      headers: creds.headers
    });
    ensureOk(response);
    const payload = parseJson(response.body) as Partial<AdminAppDetail> | undefined;
    if (!payload?.id || !payload.slug || !payload.org_slug) {
      throw new CliError(`GET /api/admin/apps/${app} did not return id, slug and org_slug`, {
        code: ExitCode.UNAVAILABLE
      });
    }
    return {
      appId: payload.id,
      label: `${payload.org_slug}/${payload.slug}`,
      orgSlug: payload.org_slug,
      appSlug: payload.slug
    };
  }

  const [orgSlug, ...rest] = app.split("/");
  const appSlug = rest.join("/");
  if (!orgSlug || !appSlug) {
    throw usageError(
      `"${app}" is not a valid app`,
      '<app> is "<org-slug>/<app-slug>" or an app UUID'
    );
  }

  let offset = 0;
  for (let page = 0; page < MAX_APP_PAGES; page++) {
    const response = await request({
      target: creds.target,
      path: `/api/admin/apps?limit=100&offset=${offset}`,
      method: "GET",
      bearer: creds.bearer,
      headers: creds.headers
    });
    ensureOk(response);
    const payload = parseJson(response.body) as AdminAppsPage | undefined;
    const match = (payload?.items ?? []).find(
      (item) => item.org_slug === orgSlug && item.slug === appSlug
    );
    if (match) {
      return { appId: match.id, label: `${orgSlug}/${appSlug}`, orgSlug, appSlug };
    }
    if (payload?.next_offset == null) break;
    offset = payload.next_offset;
  }

  throw new CliError(`no app "${app}"`, {
    code: ExitCode.NOT_FOUND,
    hint: '<app> is "<org-slug>/<app-slug>" or an app UUID'
  });
}
