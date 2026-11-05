/**
 * `oxyc proxy --app <org>/<app> --app-env <env>` — the app-scoped half of the
 * proxy: which requests are the app's own page, and what rides on them.
 *
 * The proxy has always forwarded any path. Naming an app adds one rule for the
 * requests under that app's base, `/customer-apps/<org>/<app>/`: they carry
 * `X-Oxy-App-Env`, so the server resolves the page, its assets, its beacon and
 * its `/fn` calls in that environment, and they lose the browser's cookie, so
 * the token is the only credential. Everything else is forwarded exactly as
 * before.
 *
 * Pure functions, no I/O — `proxy.ts` calls them per request.
 */

import { parseAppEnv } from "../apps/environment.js";
import { requireOwnSandbox } from "../apps/sandbox-token.js";
import { usageError } from "../util/errors.js";

/** The app a proxy is scoped to, and the environment its page is loaded in (never production). */
export interface ProxyApp {
  org: string;
  app: string;
  /** `staging` or `dev-<handle>`; absent when only `--app` was given. */
  env?: string;
}

const SLUG_RE = /^[A-Za-z0-9][A-Za-z0-9._-]*$/;

/**
 * Validate `--app` / `--app-env` together, and with the credential, before
 * anything is bound or requested. `undefined` when neither was given.
 *
 * A sandbox agent token (`sandboxAgent`) reaches its own `dev-<handle>`
 * sandboxes and nothing else: everything else it could proxy the server
 * refuses, so a proxy that names no sandbox is a mistake to stop here.
 */
export function parseProxyApp(
  flags: { app?: string; appEnv?: string },
  sandboxAgent: boolean
): ProxyApp | undefined {
  const env = flags.appEnv === undefined ? undefined : parseAppEnv(flags.appEnv);
  if (env === "production") {
    throw usageError(
      "--app-env production is not a proxy environment",
      "reads through the proxy already hit production — omit --app-env"
    );
  }
  if (env !== undefined && flags.app === undefined) {
    throw usageError(
      "--app-env needs --app <org>/<app>",
      "the environment applies to one app's page: oxyc proxy --app acme/store --app-env dev-a1"
    );
  }
  if (sandboxAgent && env === undefined) {
    throw usageError(
      "oxyc proxy needs --app <org>/<app> --app-env dev-<handle> with a sandbox agent token",
      "the token reaches only the sandboxes it created, and the server refuses everything else it could proxy"
    );
  }
  if (sandboxAgent) requireOwnSandbox("oxyc proxy", env);
  if (flags.app === undefined) return undefined;

  const [org, app, ...rest] = flags.app.split("/");
  if (!org || !app || rest.length > 0 || !SLUG_RE.test(org) || !SLUG_RE.test(app)) {
    throw usageError(
      `--app ${JSON.stringify(flags.app)} is not <org>/<app>`,
      "e.g. --app acme/store"
    );
  }
  return { org, app, env };
}

/**
 * The path the upstream request will actually name.
 *
 * The proxy forwards `fetch(`${target}${path}`)`, and the WHATWG URL parser
 * collapses `..` and `%2e%2e` segments on the way. Every decision — under the
 * app's base or not, held or forwarded — must be made on that same path, or
 * `/customer-apps/acme/store/../../../api/x` passes as the app's own request
 * and lands on `/api/x` with the token. So the handler normalises once, here,
 * and uses the result both to decide and to forward. Parsed against a fixed
 * origin by concatenation, as the forward is, so a leading `//` stays a path.
 */
export function normalizeRequestPath(raw: string): string {
  const url = new URL(`http://oxyc-proxy.invalid${raw.startsWith("/") ? raw : `/${raw}`}`);
  return `${url.pathname}${url.search}`;
}

/** `/customer-apps/<org>/<app>` — the base every request of the app's page sits under. */
export function appBase(app: ProxyApp): string {
  return `/customer-apps/${app.org}/${app.app}`;
}

/**
 * Whether `path` is the app's base or something under it. Matched whole-segment
 * (`/customer-apps/acme/store-2/` is another app) and without case, because the
 * server resolves slugs without it: a miss here would load the page in
 * production behind a path the server reads as this app's.
 */
export function isUnderApp(path: string, app: ProxyApp): boolean {
  const bare = (path.split(/[?#]/)[0] ?? path).toLowerCase();
  const base = appBase(app).toLowerCase();
  return bare === base || bare.startsWith(`${base}/`);
}

/** `localhost`, `*.localhost`, `127.0.0.0/8` or `::1` — a name only this machine answers. */
function isLoopbackHostname(hostname: string): boolean {
  const host = hostname.toLowerCase().replace(/^\[|\]$/g, "");
  return (
    host === "localhost" ||
    host.endsWith(".localhost") ||
    host === "::1" ||
    /^127(\.\d{1,3}){3}$/.test(host)
  );
}

/** Whether a URL-ish header value (`Origin`, `Referer`) names a loopback page. */
function isLoopbackUrl(value: string): boolean {
  try {
    return isLoopbackHostname(new URL(value).hostname);
  } catch {
    return false;
  }
}

/**
 * Whether a request under the app's base may carry the proxy's token.
 *
 * There the proxy drops `Origin` and `Referer` — the cloud's origin gate would
 * refuse a loopback page — so it must ask the gate's question itself, or any
 * site open in the same browser could post to `127.0.0.1:<port>` and act with
 * the token. Admitted: a request that arrived at a loopback `Host` (a page on
 * `evil.example` re-pointed at 127.0.0.1 still sends its own name), whose
 * `Origin` — or, without one, `Referer` — is a loopback page. A request with
 * neither, as `curl` or an agent's client sends, is admitted: a browser always
 * sends `Origin` on a cross-site write, and `Sec-Fetch-Site: cross-site` on
 * every request another site starts, which is refused whatever else it says.
 *
 * Any loopback origin, not only this proxy's own: a dev server on another
 * port (`pnpm dev` on 5173) calls the proxy from its page. It is the pair the
 * server's own gate admits (`oxy_app_core::origin::is_local_dev_pair`); a
 * hostile loopback page needs code already serving on this machine.
 */
export function admitsLocalRequest(headers: NodeJS.Dict<string | string[]>): boolean {
  const first = (name: string) => {
    const value = headers[name];
    return Array.isArray(value) ? value[0] : value;
  };
  const host = first("host");
  if (host === undefined || !isLoopbackUrl(`http://${host}`)) return false;
  // A browser says itself where a request came from, on every request — the
  // one signal a page cannot strip the way it can its Referer.
  if (first("sec-fetch-site")?.toLowerCase() === "cross-site") return false;
  const origin = first("origin");
  if (origin !== undefined) return origin !== "null" && isLoopbackUrl(origin);
  const referer = first("referer");
  return referer === undefined || isLoopbackUrl(referer);
}

/**
 * Whether a side-effecting call goes through without `--allow-writes`.
 *
 * Only the app's own requests in a `dev-<handle>` sandbox: its writes land in
 * the sandbox's own homes or are held server-side, which is what the proxy's
 * hold-by-default exists to guarantee for production data. They are scoped to
 * the app base because that is where `X-Oxy-App-Env` is set — a write to
 * `/api/…` carries no environment and would reach whatever the token can.
 * Staging holds, as production does.
 */
export function forwardsWritesByDefault(app: ProxyApp | undefined, path: string): boolean {
  return app?.env?.startsWith("dev-") === true && isUnderApp(path, app);
}
