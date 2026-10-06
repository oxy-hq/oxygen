/**
 * The three HTTP calls a publish makes, plus the trusted-publishing exchange.
 *
 * Direct `fetch` rather than `api/request.ts`: the upload is multipart and the
 * two lookups are public, and neither fits a JSON-string request with a
 * credential chosen by path. What they share with it is the exit-code mapping.
 */

import { withUserAgent } from "../api/user-agent.js";
import { readRefusal, sandboxRefusalHint, withReason } from "../apps/sandbox-token.js";
import { requestGithubIdToken } from "../auth/oidc.js";
import { isSandboxAgentToken } from "../auth/token-kind.js";
import { CliError, ExitCode, exitCodeForStatus } from "../util/errors.js";
import { printableLines } from "../util/printable.js";

const LOOKUP_TIMEOUT_MS = 30_000;
/** The bundle upload, which can be tens of megabytes. */
const UPLOAD_TIMEOUT_MS = 120_000;

async function send(url: string, init: RequestInit, timeoutMs: number): Promise<Response> {
  try {
    return await fetch(url, {
      ...init,
      // Every call here goes to the deployment, the two public lookups included.
      headers: withUserAgent(init.headers as Record<string, string> | undefined),
      signal: AbortSignal.timeout(timeoutMs)
    });
  } catch (cause) {
    throw new CliError(`${init.method ?? "GET"} ${url} failed: ${(cause as Error).message}`, {
      code: ExitCode.UNAVAILABLE
    });
  }
}

async function json<T>(response: Response, what: string): Promise<T> {
  const text = await response.text();
  try {
    return JSON.parse(text) as T;
  } catch {
    throw new CliError(`could not parse the ${what} response`, {
      code: ExitCode.UNAVAILABLE,
      detail: text.slice(0, 2000)
    });
  }
}

function base(target: string): string {
  return target.replace(/\/+$/, "");
}

/** The project an app publishes into, from its `(org, app)` on the target. Public. */
export async function fetchProject(target: string, org: string, app: string): Promise<string> {
  const url = `${base(target)}/api/apps/${encodeURIComponent(org)}/${encodeURIComponent(app)}/build-config`;
  const response = await send(url, {}, LOOKUP_TIMEOUT_MS);
  if (response.status === 404) {
    throw new CliError(`app ${org}/${app} is not registered on ${target}`, {
      code: ExitCode.NOT_FOUND,
      hint: "register it in the oxy admin UI, or pass --project <uuid> for its first publish"
    });
  }
  if (!response.ok) {
    throw new CliError(`build-config lookup failed (${response.status}) at ${url}`, {
      code: exitCodeForStatus(response.status)
    });
  }
  return (await json<{ project_id: string }>(response, "build-config")).project_id;
}

/** The org a pinned workspace belongs to — a workspace has exactly one. Public. */
export async function fetchOrgForProject(target: string, projectId: string): Promise<string> {
  const url = `${base(target)}/api/org-for-project/${encodeURIComponent(projectId)}`;
  const response = await send(url, {}, LOOKUP_TIMEOUT_MS);
  if (response.status === 404) {
    throw new CliError(`workspace ${projectId} not found on ${target}`, {
      code: ExitCode.NOT_FOUND,
      hint: "check --project / OXY_PROJECT"
    });
  }
  if (!response.ok) {
    throw new CliError(`org-for-project lookup failed (${response.status}) at ${url}`, {
      code: exitCodeForStatus(response.status)
    });
  }
  return (await json<{ org_slug: string }>(response, "org-for-project")).org_slug;
}

/** The server's `PublishResult`. Extra fields are tolerated. */
export interface PublishResult {
  app_id: string;
  build_id: string;
  url: string;
  channel: string;
  org_slug?: string;
  is_new_app?: boolean;
  warnings?: string[];
  /** Set when the publish carried `environment` — `staging`, `production` or a sandbox. */
  environment?: string;
  /** The environment's own host, when one is configured; null otherwise. */
  environment_url?: string | null;
}

export interface UploadRequest {
  target: string;
  token: string;
  /** Multipart text fields, in order. Undefined values are left out. */
  fields: Array<[string, string | undefined]>;
  tarball: Buffer;
}

/**
 * POST the bundle. Resolves the parsed result, or throws with the status
 * mapped onto the exit-code contract and the server's body as detail.
 */
export async function uploadBundle(req: UploadRequest): Promise<PublishResult> {
  const form = new FormData();
  for (const [name, value] of req.fields) {
    if (value !== undefined) form.append(name, value);
  }
  // A copy into a plain ArrayBuffer: `Blob` does not accept a Buffer that may
  // sit on a SharedArrayBuffer, which is what the type of `gzipSync`'s result allows.
  const bytes = new Uint8Array(req.tarball);
  form.append("bundle", new Blob([bytes], { type: "application/gzip" }), "bundle.tar.gz");

  const url = `${base(req.target)}/api/customer-apps/publish`;
  const response = await send(
    url,
    { method: "POST", headers: { authorization: `Bearer ${req.token}` }, body: form },
    UPLOAD_TIMEOUT_MS
  );
  if (!response.ok) throw publishRefused(response.status, await response.text(), req.token);
  return json<PublishResult>(response, "publish");
}

/**
 * The error for a refused publish. The route answers plain text, except the
 * one refusal a sandbox agent token branches on (`403 sandbox_token_refused`),
 * which is JSON with a `code`.
 *
 * Under a sandbox agent token the server's sentence goes on the error line as
 * well as in `detail`: `oxyc mcp` hands a model the message and the hint and
 * never the body, and a publish that failed without saying why cannot be fixed.
 */
function publishRefused(status: number, body: string, token: string): CliError {
  const refusal = readRefusal(body);
  const agent = isSandboxAgentToken(token);
  const headline = `publish failed (${status})`;
  return new CliError(agent ? withReason(headline, refusal) : headline, {
    code: exitCodeForStatus(status),
    detail: agent ? printableLines(body.slice(0, 4000)) : body.slice(0, 4000),
    hint: agent ? sandboxRefusalHint(status, body) : undefined,
    serverCode: refusal.code,
    serverMessage: refusal.reason
  });
}

export { githubOidcAvailable } from "../auth/oidc.js";

/**
 * The audience the APP-SCOPED exchange pins — not the general one, which is
 * the deployment's own, `oxy:<host>` (`auth/oidc.ts`). Each endpoint refuses the other's audience, so a
 * publish-audience token can never be traded for a broader credential.
 */
export const PUBLISH_OIDC_AUDIENCE = "oxy-publish";

/**
 * The app-scoped exchange: a GitHub OIDC token, traded for a publish
 * credential scoped to exactly `org/app` and valid for fifteen minutes.
 *
 * THE OLDER OF THE TWO EXCHANGES, and now the fallback. `auth/oidc.ts` is
 * tried first; this one is reached when the deployment has no general exchange
 * (404), or has one and no trust policy matches the run — in which case the
 * app may still have a publisher registered the older way. It stays because
 * every workflow `oxyc init-ci` wrote before trust policies existed depends on
 * it, and so does a deployment that has not been upgraded.
 *
 * Called immediately before the upload, not at startup: the OIDC token is
 * single-use and the credential short-lived, and a build can take longer than
 * either should be held. The server matches the token's claims against a
 * publisher registered for the app — repository owner id, repo, workflow path,
 * environment — so a 403 here almost always means a registration mismatch.
 */
export interface ExchangedCredential {
  /** The short-lived, app-scoped publish token. */
  token: string;
  /**
   * The app it is scoped to, so a caller never looks one up — absent on a
   * deployment predating trusted checks. Optional on purpose: publishing does
   * not need it, and a `publish` that refused to run because a field it never
   * reads was missing would be a new failure on the load-bearing path. The
   * caller that needs it says so.
   */
  appId?: string;
}

export async function exchangeGithubOidc(
  target: string,
  orgSlug: string,
  app: string,
  env: NodeJS.ProcessEnv = process.env
): Promise<ExchangedCredential> {
  // A token of its own, never one left over from the general exchange: each
  // is single-use and minted for one audience.
  const oidc = await requestGithubIdToken(PUBLISH_OIDC_AUDIENCE, env);

  const url = `${base(target)}/api/customer-apps/publish/oidc-exchange`;
  const exchanged = await send(
    url,
    {
      method: "POST",
      // `Bearer` exactly: the server matches the prefix case-sensitively.
      headers: { authorization: `Bearer ${oidc}`, "content-type": "application/json" },
      body: JSON.stringify({ app: `${orgSlug}/${app}` })
    },
    LOOKUP_TIMEOUT_MS
  );
  if (!exchanged.ok) {
    const body = await exchanged.text();
    throw new CliError(`trusted-publishing exchange failed (${exchanged.status})`, {
      code: exitCodeForStatus(exchanged.status),
      detail: body.slice(0, 2000),
      hint: `is this workflow registered as a publisher for ${orgSlug}/${app}, with the same repository, workflow file and environment?`
    });
  }
  const body = await json<{ token?: string; app_id?: string }>(exchanged, "exchange");
  if (!body.token) throw new CliError("the exchange returned no token", { code: ExitCode.AUTH });
  return { token: body.token, appId: body.app_id };
}
