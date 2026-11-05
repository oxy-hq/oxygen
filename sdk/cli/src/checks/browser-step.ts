/**
 * `checks run --browser`'s step: turn the resolved app, credential and
 * environment into a {@link BrowserTarget}, and run it.
 *
 * Outside production it first reads the environment (`GET …/environments/{name}`)
 * for the build it serves, so the page can be held to that build; an
 * environment that serves none, or is being deleted, is refused here rather
 * than loaded into a 404.
 */

import { APP_ENV_HEADER, isProduction } from "../apps/environment.js";
import type { Creds, ResolvedApp } from "../apps/resolve.js";
import { isMachineIdentity } from "../auth/token-kind.js";
import { fetchEnvironment } from "../commands/env.js";
import type { Context } from "../context/resolve.js";
import { CliError, ExitCode, usageError } from "../util/errors.js";
import { type BrowserCheckResult, type BrowserTarget, runBrowserCheck } from "./browser.js";
import type { PwChromium } from "./playwright.js";

export interface BrowserStepOptions {
  appEnv?: string;
  expectSelector?: string;
  /** `checks run --timeout`; each browser wait is this or 30s, whichever is shorter. */
  timeoutSeconds: number;
}

const MAX_WAIT_SECONDS = 30;

/**
 * `--browser` loads the app AS A USER, which a machine token — a publish,
 * service-account or CI token, stored or minted from GitHub OIDC
 * (`oidcAvailable`, when no user credential is set) — cannot be: it reaches
 * the `/customer-apps` API routes only, so the page would redirect to
 * sign-in. Refused before any request, and before the exchange would mint one.
 * The same order `resolveCredentials` reads them in: a stored bearer, then an
 * API key, then OIDC.
 *
 * A SANDBOX AGENT TOKEN IS THE EXCEPTION. The server lets it load its own
 * sandbox's page on the product host's path URL when the request carries the
 * bearer and `X-Oxy-App-Env: dev-<handle>` — which is what `browserTarget`
 * sends, to requests under the app base only. Which environment it may name
 * is `requireOwnSandbox`'s call (`resolveCredentials`), made before this
 * check's first request.
 */
export function refuseMachineBrowser(ctx: Context, oidcAvailable: boolean): void {
  const bearer = ctx.storedBearer();
  const machine = bearer ? isMachineIdentity(bearer) : !ctx.apiKey() && oidcAvailable;
  if (machine) {
    throw usageError(
      "the browser check loads the app as a user, which this token cannot",
      "--browser needs a user credential: oxyc login, OXY_TOKEN set to a user token, or an API key"
    );
  }
}

/** The credential as request headers: the bearer, else what `creds.headers` carries (an API key). */
export function credentialHeaders(creds: Creds): Record<string, string> {
  if (creds.bearer) return { authorization: `Bearer ${creds.bearer}` };
  const headers: Record<string, string> = {};
  for (const [name, value] of Object.entries(creds.headers ?? {})) {
    headers[name.toLowerCase()] = value;
  }
  return headers;
}

/** The target for `app` in `appEnv`, given the build that environment serves (if any). */
export function browserTarget(
  creds: Creds,
  app: ResolvedApp,
  opts: BrowserStepOptions,
  expectedBuild: string | undefined
): BrowserTarget {
  const environment = opts.appEnv ?? "production";
  const url = `${creds.target.replace(/\/+$/, "")}/customer-apps/${app.orgSlug}/${app.appSlug}/`;
  const headers = credentialHeaders(creds);
  if (!isProduction(opts.appEnv)) headers[APP_ENV_HEADER.toLowerCase()] = environment;
  return {
    url,
    origin: new URL(url).origin,
    basePath: new URL(url).pathname,
    headers,
    environment,
    expectedBuild,
    expectSelector: opts.expectSelector,
    waitMs: Math.min(opts.timeoutSeconds, MAX_WAIT_SECONDS) * 1000
  };
}

/** The build a non-production environment serves; refuses one that serves nothing. */
async function servedBuild(creds: Creds, app: ResolvedApp, name: string): Promise<string> {
  const environment = await fetchEnvironment(creds, app.appId, name);
  if (environment.status === "deleting") {
    throw new CliError(`${name} of ${app.label} is being deleted`, {
      code: ExitCode.NOT_FOUND,
      hint: "a deleting environment serves nothing"
    });
  }
  if (!environment.build_uuid) {
    throw new CliError(`${name} of ${app.label} serves no build yet`, {
      code: ExitCode.NOT_FOUND,
      remedy: `oxyc publish --app-env ${name}`
    });
  }
  return environment.build_uuid;
}

export async function runBrowserStep(
  creds: Creds,
  app: ResolvedApp,
  opts: BrowserStepOptions,
  load?: () => Promise<PwChromium>
): Promise<BrowserCheckResult> {
  const expectedBuild = isProduction(opts.appEnv)
    ? undefined
    : await servedBuild(creds, app, opts.appEnv as string);
  return runBrowserCheck(browserTarget(creds, app, opts, expectedBuild), load);
}
