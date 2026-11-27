/**
 * Which credential `oxyc checks run` uses, and which surface it can reach.
 *
 * Every credential reaches the three function routes at `/api/customer-apps`.
 * What the CREDENTIAL picks, not the environment, is whether the app can be
 * looked up: a publish token, a service account's token or a sandbox agent
 * token (`oxy_sbx_…`) may not list apps, so `machine` is set and the app id
 * comes from the credential or the caller instead.
 *
 * ORDER, which differs from every other command in exactly one place:
 *
 *   1. a stored bearer — `OXY_TOKEN`, then the login cache
 *   2. `OXY_API_KEY`
 *   3. GitHub OIDC — the general exchange when the run names its service
 *      account (`OXY_SERVICE_ACCOUNT`), then the app's own publisher; the
 *      publisher alone when it names none
 *
 * The API key sits AHEAD of OIDC because this command can use one on its own.
 * A job that sets `OXY_API_KEY` and also happens to hold `id-token: write` has
 * said which credential it means, and minting over the top of it would swap a
 * staff key for a service account that cannot list apps.
 */

import { isProduction } from "../apps/environment.js";
import { type Creds, type ResolvedApp, resolveApp, UUID_RE } from "../apps/resolve.js";
import { requireOwnSandbox, resolveSandboxApp } from "../apps/sandbox-token.js";
import { fallsBackToPublisher, githubOidcAvailable, OidcExchangeError } from "../auth/oidc.js";
import { introspectToken } from "../auth/token-api.js";
import { credentialShape, isMachineIdentity, isSandboxAgentToken } from "../auth/token-kind.js";
import type { Context } from "../context/resolve.js";
import { exchangeGithubOidc } from "../publish/server.js";
import { CliError, ExitCode, usageError } from "../util/errors.js";

/** Where the three function routes live, for every kind of credential. */
export const APPS_API = "/api/customer-apps";

export { UUID_RE };

export interface ResolvedCredentials {
  bearer?: string;
  apiKey?: string;
  /** True for a credential that may not list apps (publish, service account, sandbox agent). */
  machine: boolean;
  /** Set when the credential itself told us which app it is scoped to. */
  appId?: string;
}

function splitSlug(app: string): { orgSlug?: string; appSlug?: string } {
  const [orgSlug, ...rest] = app.split("/");
  return { orgSlug: orgSlug || undefined, appSlug: rest.join("/") || undefined };
}

/**
 * The app a service account's token names, when `app` is a slug.
 *
 * Read off the token's own `app_publish` grants (`GET /api/auth/token`), since
 * a machine token may not list apps. MATCHED BY NAME, never assumed: a token
 * scoped to one app is not evidence that it is the app the caller typed, and
 * running a different app's checks under this one's label would report a pass
 * for something nobody ran. No match is `undefined`, and the caller decides.
 */
async function appIdFromGrants(
  target: string,
  bearer: string,
  app: string
): Promise<string | undefined> {
  const { appSlug } = splitSlug(app);
  if (!appSlug) return undefined;
  const found = await introspectToken(target, bearer);
  if (found.kind !== "token") return undefined;
  const wanted = appSlug.toLowerCase();
  const matches = found.token.grants.filter(
    (g) =>
      g.kind === "app_publish" && !g.revoked_at && g.app_id && g.app_name?.toLowerCase() === wanted
  );
  return matches.length === 1 ? (matches[0]?.app_id ?? undefined) : undefined;
}

/** Whether a bearer is a machine credential, and the app id when the token can supply it. */
async function forBearer(
  target: string,
  bearer: string,
  app: string
): Promise<ResolvedCredentials> {
  const shape = credentialShape(bearer);
  if (shape === "publish") return { bearer, machine: true };
  if (shape === "service_account" || shape === "ci") {
    const appId = UUID_RE.test(app) ? undefined : await appIdFromGrants(target, bearer, app);
    return { bearer, machine: true, appId };
  }
  if (shape === "sandbox_agent") {
    // This token may not list apps. The app comes off the token's
    // own list — a slug or a UUID — and one it was not minted for stops here.
    const { appId } = await resolveSandboxApp(target, bearer, app);
    return { bearer, machine: true, appId };
  }
  return { bearer, machine: false };
}

/**
 * The app's own publisher exchange — audience `oxy-publish`, keyed by slug,
 * and the one exchange that hands back the app id with the token.
 */
async function fromPublisher(
  target: string,
  app: string,
  refused: OidcExchangeError | undefined
): Promise<ResolvedCredentials> {
  const { orgSlug, appSlug } = splitSlug(app);
  if (!orgSlug || !appSlug) {
    // No trust policy AND no slug to look a publisher up by: the policy is the
    // thing to fix, so its error is the one worth showing.
    if (refused?.oidcCode === "no_matching_policy") throw refused;
    throw usageError(
      'trusted publishing needs <app> as "<org-slug>/<app-slug>"',
      "the exchange is keyed by slug; a UUID names an app it cannot verify a publisher for"
    );
  }
  const minted = await exchangeGithubOidc(target, orgSlug, appSlug);
  // Only this caller needs the id — `oxyc publish` takes the token and goes.
  // So the version check is here, not in the exchange: a deployment without
  // the field can still be published to.
  if (!minted.appId) {
    throw new CliError("the OIDC exchange returned no app_id", {
      code: ExitCode.UNAVAILABLE,
      hint: "this deployment predates trusted checks — upgrade it, or set OXY_TOKEN and pass the app UUID"
    });
  }
  return { bearer: minted.token, machine: true, appId: minted.appId };
}

/**
 * Mint from the job's GitHub OIDC identity: the general exchange first, the
 * app's publisher second — the same order, for the same reasons, as
 * `oxyc publish` (`mintPublishToken`). A run that names no service account
 * never reaches the general exchange: `ctx.bearer()` raises
 * `no_service_account` without a request, and the publisher is next.
 *
 * One more way to reach the second here: the general exchange SUCCEEDED, but
 * its token cannot name the app behind a slug. The publisher exchange can —
 * it is keyed by that slug — so it is tried before telling the caller to go
 * find a UUID.
 */
async function mint(ctx: Context, target: string, app: string): Promise<ResolvedCredentials> {
  let refused: OidcExchangeError | undefined;
  let general: ResolvedCredentials | undefined;
  try {
    general = await forBearer(target, await ctx.bearer(), app);
    if (general.appId || UUID_RE.test(app)) return general;
  } catch (cause) {
    if (!(cause instanceof OidcExchangeError) || !fallsBackToPublisher(cause)) throw cause;
    refused = cause;
  }
  try {
    return await fromPublisher(target, app, refused);
  } catch (cause) {
    // The general token is real and simply cannot name the app. That is the
    // error worth showing — `runChecks` raises it — not the publisher's.
    if (general) return general;
    throw cause;
  }
}

/**
 * A stored credential if there is one, a minted one if there is not.
 *
 * Nothing resolving and nothing mintable throws the SAME `authError` every
 * other command throws — `ctx.bearer()` raises it, so the message and the
 * `oxyc login …` hint stay defined in exactly one place.
 *
 * `appEnv` is read here so a non-production refusal lands BEFORE the OIDC
 * exchange: either exchange yields a machine token (a service account's or a
 * publish token), and outside production only a staff credential is let in
 * (D22). Minting one just to throw it away would spend the single-use GitHub
 * token and could report `UNAVAILABLE` instead of the real `USAGE`. A stored
 * machine token is refused the same way, so the two paths answer alike.
 */
export async function resolveCredentials(
  ctx: Context,
  target: string,
  app: string,
  appEnv?: string
): Promise<ResolvedCredentials> {
  const refuseNonProduction = (): never => {
    throw usageError(
      `a publish or service-account token cannot run checks in ${appEnv}`,
      "sandboxes and staging need a staff credential — oxyc login, or OXY_TOKEN set to a user token"
    );
  };

  const stored = ctx.storedBearer();
  if (stored) {
    // Before `forBearer`, which may ask the server what a service account's
    // token names — a refusal makes no request at all.
    if (isMachineIdentity(stored) && !isProduction(appEnv)) refuseNonProduction();
    // The mirror image, and likewise before any request: a sandbox agent
    // token runs checks in its own sandboxes and nowhere else.
    if (isSandboxAgentToken(stored)) requireOwnSandbox("oxyc checks run", appEnv);
    return forBearer(target, stored, app);
  }

  const apiKey = ctx.apiKey();
  if (apiKey) return { apiKey, machine: false };

  if (githubOidcAvailable()) {
    if (!isProduction(appEnv)) refuseNonProduction();
    return mint(ctx, target, app);
  }

  // Throws — nothing resolved and nothing can be minted.
  await ctx.bearer();
  return { machine: false };
}

/** An app the credential named without a lookup: no slugs, which only the lookup learns. */
export interface AppByCredential {
  appId: string;
  label: string;
  orgSlug?: undefined;
  appSlug?: undefined;
}

/**
 * The app `app` names, for the credential `resolveCredentials` returned.
 *
 * `resolveApp` pages `/api/customer-apps`, which a machine token may not list:
 * sending it there would 403 on a route the caller cannot be given. A token
 * that names its own app (the publisher exchange's `app_id`, a service
 * account's `app_publish` grant, a sandbox agent token's list) never gets
 * there; a machine token that does not needs the UUID — and is never sent to
 * `GET /api/customer-apps/{id}` either, which refuses it just as the listing
 * does and would add nothing the id does not already say. Only the lookup
 * learns the slugs, so only a user credential's answer carries them.
 */
export async function appForCredentials(
  creds: Creds,
  resolved: ResolvedCredentials,
  app: string
): Promise<ResolvedApp | AppByCredential> {
  if (resolved.appId) return { appId: resolved.appId, label: app };
  if (!resolved.machine) return resolveApp(creds, app);
  if (UUID_RE.test(app)) return { appId: app, label: app };
  throw usageError(
    "this token cannot resolve <org>/<app> — pass the app UUID",
    "resolving a slug means listing every app, which a publish or service-account token may not do; the id is on the app's admin page, and `oxyc publish --json` reports it"
  );
}
