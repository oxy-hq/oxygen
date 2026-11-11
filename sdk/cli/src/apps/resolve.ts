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
import { isMachineIdentity, isSandboxAgentToken } from "../auth/token-kind.js";
import { type Context, notAuthenticated } from "../context/resolve.js";
import { CliError, ExitCode, usageError } from "../util/errors.js";
import { resolveSandboxApp, sandboxTokenError } from "./sandbox-token.js";

export { SANDBOX_TOKEN_PREFIX } from "../auth/token-kind.js";

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
  // Nothing resolved — the canonical error, naming the login command or, when
  // a named variable is the only source, the variable.
  throw notAuthenticated(ctx.target(), ctx.flags);
}

/**
 * Throw for a non-2xx response. Pass the credential that made the request:
 * under a sandbox agent token the error carries what that caller can act on,
 * in place of the generic "try `oxyc login` again".
 */
export function ensureOk(response: ApiResponse, creds?: { bearer?: string }): void {
  if (response.status >= 200 && response.status < 300) return;
  if (isSandboxAgentToken(creds?.bearer)) throw sandboxTokenError(response);
  if (isBareRefusal(response)) throw bareRefusal(response);
  throw errorForResponse(response);
}

/**
 * A bare `403` with no body: how the staff app console's guards refuse —
 * `block_admin_while_acting` (an active assume-role session), and the
 * standing and `PlatformApps` guards (no grant for this console). The CLI
 * cannot tell which, so the hint names both. Refusals that say why in a
 * body are left as they are.
 */
function isBareRefusal(response: ApiResponse): boolean {
  return response.status === 403 && response.body.trim() === "";
}

/** `errorForResponse`, with the two reasons the console refuses this way. */
function bareRefusal(response: ApiResponse): CliError {
  const base = errorForResponse(response);
  return new CliError(base.message, {
    code: base.code,
    hint:
      "the staff app console refused: either you are acting as a tenant (`oxyc assume status`), " +
      "which it refuses outright, or your staff grant does not include this console",
    remedy:
      "if you are in an assume session, name the app as <org-slug>/<app-slug> — `oxyc logs` and " +
      "`oxyc invocations list` read it by slug there — or end the session with `oxyc assume end`"
  });
}

/** The app registry API: where every credential lists, reads and runs an app. */
export const CUSTOMER_APPS_API = "/api/customer-apps";

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
 * The org/app form is resolved by paging `GET /api/customer-apps` — the matched
 * row already carries `org_slug` and `slug`, so no extra request. A UUID
 * needs one more call, `GET /api/customer-apps/{id}`, to learn them — the shape
 * `fn call` and `logs` need for their `/customer-apps/{org}/{app}/...`
 * routes. Both calls need a credential that may list apps: a publish token never reaches
 * this function (`checks.ts` short-circuits around it for that credential;
 * every other caller goes through `staffCreds`, which already refused one).
 *
 * A SANDBOX AGENT TOKEN MAY NOT LIST APPS EITHER, and resolves
 * against its own app list instead (`GET /api/auth/token`).
 */
export async function resolveApp(creds: Creds, app: string): Promise<ResolvedApp> {
  if (isSandboxAgentToken(creds.bearer)) {
    return resolveSandboxApp(creds.target, creds.bearer, app);
  }
  if (UUID_RE.test(app)) {
    const response = await request({
      target: creds.target,
      path: `${CUSTOMER_APPS_API}/${app}`,
      method: "GET",
      bearer: creds.bearer,
      headers: creds.headers
    });
    ensureOk(response);
    const payload = parseJson(response.body) as Partial<AdminAppDetail> | undefined;
    if (!payload?.id || !payload.slug || !payload.org_slug) {
      throw new CliError(`GET ${CUSTOMER_APPS_API}/${app} did not return id, slug and org_slug`, {
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

  const { orgSlug, appSlug } = parseAppSlugs(app);

  let offset = 0;
  for (let page = 0; page < MAX_APP_PAGES; page++) {
    const response = await request({
      target: creds.target,
      path: `${CUSTOMER_APPS_API}?limit=100&offset=${offset}`,
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

/** Split `<org-slug>/<app-slug>`; anything else is a usage error. */
function parseAppSlugs(app: string): { orgSlug: string; appSlug: string } {
  const [orgSlug, ...rest] = app.split("/");
  const appSlug = rest.join("/");
  if (!orgSlug || !appSlug) {
    throw usageError(
      `"${app}" is not a valid app`,
      '<app> is "<org-slug>/<app-slug>" or an app UUID'
    );
  }
  return { orgSlug, appSlug };
}

/**
 * The slugs of `app`, for a route addressed by `/customer-apps/{org}/{app}/...`
 * (`logs`). An `<org-slug>/<app-slug>` argument already IS them, so it makes
 * no request: the registry walk in `resolveApp` sits on the staff console
 * stack, which refuses a caller inside an assume-role session and filters out
 * an app beyond an App Operator's scope — while the slug route decides its own
 * authority (`Ring::AppAdmin`) and answers for the app itself. A UUID still
 * needs `resolveApp` to learn its slugs, and a sandbox agent token still
 * resolves against its own app list.
 */
export async function resolveAppSlugs(
  creds: Creds,
  app: string
): Promise<Pick<ResolvedApp, "label" | "orgSlug" | "appSlug">> {
  if (isSandboxAgentToken(creds.bearer) || UUID_RE.test(app)) {
    return resolveApp(creds, app);
  }
  const { orgSlug, appSlug } = parseAppSlugs(app);
  return { label: `${orgSlug}/${appSlug}`, orgSlug, appSlug };
}
