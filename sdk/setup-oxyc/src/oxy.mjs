/**
 * The three network calls: ask GitHub for an OIDC token, trade it for an Oxy
 * token, and revoke that token afterwards. The token's audience is worked out
 * from the deployment's URL ({@link audienceFor}); the deployment is not asked.
 *
 * The exchange is `POST <host>/api/auth/oidc/exchange` with
 * `{ token, service_account }`, where `service_account` is the account's ID.
 * The deployment verifies the GitHub token against the trust policies of
 * THAT service account — the one the workflow names, and no other — and, on
 * a match, answers
 * `{ token: "oxy_ci_…", token_id, expires_at, service_account }` — a
 * credential good for fifteen minutes. `DELETE <host>/api/auth/token`,
 * authenticated with that credential, ends it sooner.
 *
 * `oxyc` does the same exchange on its own when no `OXY_TOKEN` is set
 * (`sdk/cli/src/auth/oidc.ts`); the audience and the error codes here must
 * stay in step with it.
 */

import { mask } from "./io.mjs";

/** @typedef {import("./io.mjs").Io} Io */

/**
 * The audience of the deployment at `host`: `oxy:<its host>`.
 *
 * WORKED OUT FROM THE URL, NEVER ASKED FOR. A GitHub OIDC token is good
 * wherever its audience is accepted, and each deployment accepts the audience
 * of the address it calls itself by. So a token asked for THE HOST IT IS ABOUT
 * TO BE POSTED TO is good at that deployment and nowhere else. Were the
 * deployment asked which audience to use, a compromised or look-alike one
 * could name another deployment's and be handed a token that one accepts; were
 * there a fallback to a shared audience, any server could trigger it. Neither
 * exists, and plain `oxy` is never asked for.
 *
 * `URL.host`: lowercased, with `:<port>` only when it is not the scheme's
 * default — exactly what the server derives from its public URL
 * (`deployment_audience`, `crates/auth`) and what `oxyc` derives
 * (`oidcAudience`, `sdk/cli/src/auth/oidc.ts`). The cases in the tests are
 * repeated in all three so they cannot drift.
 *
 * @param {string} host The deployment's base URL: the validated `host` input.
 */
export function audienceFor(host) {
  return `oxy:${new URL(host).host}`;
}

const TIMEOUT_MS = 30_000;
/** Pauses before the second and third attempt at the exchange. */
const RETRY_DELAYS_MS = [1_000, 3_000];
/**
 * The longest a rate-limited exchange waits before its one retry. The
 * deployment's budget refills at one request a second, so its `Retry-After` is
 * almost always `1`. Keep in step with `sdk/cli/src/auth/oidc.ts`.
 */
export const RATE_LIMIT_MAX_WAIT_S = 60;

/**
 * `Retry-After` as whole seconds to wait, capped. Missing or unreadable is one.
 * @param {string | null | undefined} header
 */
export function retryAfterSeconds(header) {
  const seconds = Number.parseInt(header ?? "", 10);
  if (!Number.isFinite(seconds) || seconds < 1) return 1;
  return Math.min(seconds, RATE_LIMIT_MAX_WAIT_S);
}

/** A failure with the lines that say what to do about it. */
export class SetupError extends Error {
  /**
   * @param {string} message
   * @param {string[]} [hints]
   */
  constructor(message, hints = []) {
    super(message);
    this.name = "SetupError";
    this.hints = hints;
  }
}

/**
 * @typedef {object} Minted
 * @property {"minted"} kind
 * @property {string} token
 * @property {string} tokenId
 * @property {string} expiresAt
 * @property {string} serviceAccount
 */

/** @typedef {{ kind: "unsupported" }} Unsupported The deployment has no exchange. */

/** The two variables GitHub sets in a job granted `id-token: write`. */
export function oidcAvailable(/** @type {NodeJS.ProcessEnv} */ env) {
  return Boolean(env.ACTIONS_ID_TOKEN_REQUEST_URL && env.ACTIONS_ID_TOKEN_REQUEST_TOKEN);
}

const NEEDS_PERMISSION =
  "add `permissions: id-token: write` (and `contents: read`) to the job. A job-level `permissions` block replaces the workflow-level one, so list both.";

/** Fail, with the fix, in a job that was not granted `id-token: write`. */
export function requireOidc(/** @type {NodeJS.ProcessEnv} */ env) {
  if (!oidcAvailable(env)) {
    throw new SetupError("this job cannot request a GitHub OIDC token", [NEEDS_PERMISSION]);
  }
}

/** @param {unknown} cause */
function reason(cause) {
  return cause instanceof Error ? cause.message : String(cause);
}

/**
 * Ask GitHub for an id token for `audience`. Single-use on the Oxygen side —
 * the deployment records its `jti` — so every attempt at the exchange asks for
 * a fresh one.
 *
 * @param {Io} io
 * @param {string} audience The deployment's own: {@link audienceFor}.
 */
export async function requestIdToken(io, audience) {
  requireOidc(io.env);
  const url = new URL(io.env.ACTIONS_ID_TOKEN_REQUEST_URL ?? "");
  url.searchParams.set("audience", audience);
  /** @type {Response} */
  let response;
  try {
    response = await io.fetch(url, {
      headers: { authorization: `bearer ${io.env.ACTIONS_ID_TOKEN_REQUEST_TOKEN}` },
      signal: AbortSignal.timeout(TIMEOUT_MS)
    });
  } catch (cause) {
    throw new SetupError(`could not reach GitHub's OIDC endpoint: ${reason(cause)}`);
  }
  if (!response.ok) {
    throw new SetupError(`GitHub refused to mint an OIDC token (${response.status})`, [
      NEEDS_PERMISSION
    ]);
  }
  const body = /** @type {{ value?: unknown } | null} */ (await response.json().catch(() => null));
  const value = typeof body?.value === "string" ? body.value : "";
  if (!value) throw new SetupError("GitHub returned no OIDC token");
  // Short-lived and audience-bound, but still a credential: keep it out of logs.
  mask(io, value);
  return value;
}

/** What this run looks like to a trust policy, from the variables GitHub sets. */
function runIdentity(/** @type {NodeJS.ProcessEnv} */ env) {
  return [
    env.GITHUB_REPOSITORY ? `repository: ${env.GITHUB_REPOSITORY}` : "",
    env.GITHUB_WORKFLOW_REF ? `workflow: ${env.GITHUB_WORKFLOW_REF}` : "",
    env.GITHUB_REF ? `ref: ${env.GITHUB_REF}` : "",
    env.GITHUB_EVENT_NAME ? `event: ${env.GITHUB_EVENT_NAME}` : ""
  ].filter(Boolean);
}

/**
 * One refused exchange, as an error a person can act on. The server's `code`
 * picks the wording; its `error` string rides along as a hint.
 *
 * @param {number} status
 * @param {{ error?: unknown; code?: unknown; audience?: unknown } | null} body
 * @param {NodeJS.ProcessEnv} env
 * @param {string} [host] The deployment the exchange was posted to.
 */
export function explain(status, body, env, host = "") {
  const said = typeof body?.error === "string" && body.error.trim() ? [body.error.trim()] : [];
  const code = typeof body?.code === "string" ? body.code : "";
  if (status === 400 && !code) {
    // The one refusal with no code: the body itself was unusable.
    return new SetupError("the deployment could not read the exchange request", [
      ...said,
      "the request did not arrive as it was sent. Re-run the job; if it keeps failing, something between the runner and the deployment is rewriting request bodies."
    ]);
  }
  switch (code) {
    case "invalid_token":
      return new SetupError("the deployment rejected the GitHub OIDC token", [
        ...said,
        "its signature or issuer did not verify. Re-run the job; if it keeps failing, the deployment cannot reach GitHub's signing keys."
      ]);
    case "wrong_audience": {
      // The deployment takes another audience than the one derived from
      // `host`, so it calls itself by another address. The fix is always the
      // `host` input, never which audience is asked for.
      if (body?.audience === "oxy") {
        return new SetupError("GitHub sign-in is not available on this deployment", [
          ...said,
          `${host || "it"} has no public URL configured (OXY_API_URL), so it has no address of its own to accept a GitHub token for — and this action never asks GitHub for a token any deployment would take. Set OXY_TOKEN from a secret, or have the deployment's operator set OXY_API_URL.`
        ]);
      }
      const theirs =
        typeof body?.audience === "string" && body.audience.startsWith("oxy:")
          ? body.audience.slice(4)
          : "";
      return new SetupError(
        "the GitHub OIDC token was minted for the wrong audience: this deployment answers to another address",
        [
          ...said,
          theirs
            ? `this deployment answers to ${theirs}, and the token is asked for the host it is about to be sent to${host ? ` (here ${new URL(host).host})` : ""}. Set the \`host\` input to https://${theirs}.`
            : "the token is asked for the host it is about to be sent to, and this deployment calls itself by another. Set the `host` input to the deployment's own address."
        ]
      );
    }
    case "expired":
      return new SetupError("the GitHub OIDC token expired before it was exchanged", [
        ...said,
        "re-run the job. A large clock skew on a self-hosted runner produces the same answer."
      ]);
    case "replayed":
      return new SetupError("the GitHub OIDC token had already been used", [
        ...said,
        "each token is single-use. Re-run the job."
      ]);
    case "pull_request_target":
      return new SetupError("refused: this run was triggered by `pull_request_target`", [
        ...said,
        "that event runs a fork's changes with the base repository's identity, so no trust policy matches it. Trigger on `push`, `pull_request` or `workflow_dispatch`."
      ]);
    case "self_hosted_runner":
      return new SetupError("refused: the job ran on a self-hosted runner", [
        ...said,
        "the trust policy allows GitHub-hosted runners only. Use a hosted runner, or have an org admin allow self-hosted runners on the policy."
      ]);
    case "missing_environment":
      // Two ways to get here, and the server's words say which: the job
      // names no environment and the policy needs one, or the policy names
      // none while its organization requires every policy to.
      return new SetupError("refused: a GitHub environment is required and missing", [
        ...said,
        "if the job declares no `environment:`, add `environment: <name>` to the job, with the name the trust policy was registered under.",
        "If it does, the trust policy itself names no environment while the organization requires one: an org admin sets it on the policy."
      ]);
    case "no_matching_policy":
      return new SetupError(
        "no trust policy of the named service account matches this workflow run",
        [
          ...said,
          ...runIdentity(env),
          "check the `service-account` input, and register a policy on that account: in the web app under Organization settings → API access → Service accounts → (the account) → Trusted access, or with `oxyc init-ci` from the repository.",
          "The repository, the workflow file and the job's `environment:` must all match the policy."
        ]
      );
    case "service_account_required":
      return new SetupError("the service account must be named by its ID", [
        ...said,
        "set the `service-account` input to the account's ID (a UUID), never <org-slug>/<name>."
      ]);
    case "rate_limited":
      return new SetupError("the deployment is rate-limiting token exchanges from this runner", [
        ...said,
        "the exchange allows 60 requests a minute per client address, and this step already waited once and retried. Re-run the job."
      ]);
    default:
      return new SetupError(`the OIDC token exchange failed (${status})`, said);
  }
}

/**
 * Trade this job's GitHub identity for an Oxy token.
 *
 * A network failure or a 5xx is retried twice, each time with a FRESH id
 * token; anything the deployment actually decided is final — except a 429,
 * which is waited out once for its `Retry-After` (capped at
 * {@link RATE_LIMIT_MAX_WAIT_S}) and retried, also with a fresh id token. A 404
 * is not an error here but an answer — the deployment has no exchange — and
 * the caller decides what that means.
 *
 * @param {Io} io
 * @param {string} host
 * @param {string} serviceAccount The account's ID. Always sent: the
 *   deployment refuses a request that names no account, or names one any
 *   other way.
 * @returns {Promise<Minted | Unsupported>}
 */
export async function exchange(io, host, serviceAccount) {
  requireOidc(io.env);
  // From the URL the token is about to be posted to — no request, and
  // nothing the deployment says can change it.
  const audience = audienceFor(host);
  const url = `${host}/api/auth/oidc/exchange`;
  let failures = 0;
  let waitedOut = false;
  for (;;) {
    const idToken = await requestIdToken(io, audience);
    const delay = RETRY_DELAYS_MS[failures];
    /** @type {Response} */
    let response;
    try {
      response = await io.fetch(url, {
        method: "POST",
        headers: { "content-type": "application/json", accept: "application/json" },
        body: JSON.stringify({ token: idToken, service_account: serviceAccount }),
        signal: AbortSignal.timeout(TIMEOUT_MS)
      });
    } catch (cause) {
      if (delay === undefined) throw new SetupError(`could not reach ${url}: ${reason(cause)}`);
      failures++;
      await io.sleep(delay);
      continue;
    }
    if (response.status >= 500 && delay !== undefined) {
      failures++;
      await io.sleep(delay);
      continue;
    }
    if (response.status === 429 && !waitedOut) {
      waitedOut = true;
      const wait = retryAfterSeconds(response.headers.get("retry-after"));
      await response.arrayBuffer().catch(() => undefined);
      await io.sleep(wait * 1000);
      continue;
    }
    if (response.status === 404) return { kind: "unsupported" };

    const body = /** @type {Record<string, unknown> | null} */ (
      await response.json().catch(() => null)
    );
    if (!response.ok) throw explain(response.status, body, io.env, host);

    const token = typeof body?.token === "string" ? body.token : "";
    if (!token) throw new SetupError("the OIDC exchange returned no token");
    // Before anything else can print it.
    mask(io, token);
    return {
      kind: "minted",
      token,
      tokenId: typeof body?.token_id === "string" ? body.token_id : "",
      expiresAt: typeof body?.expires_at === "string" ? body.expires_at : "",
      serviceAccount: typeof body?.service_account === "string" ? body.service_account : ""
    };
  }
}

/**
 * @typedef {"revoked" | "already_invalid" | "unsupported" | "failed"} RevokeOutcome
 */

/**
 * Revoke the token that makes the call. Never throws: this runs in the post
 * step, where a failure would turn a finished job red over a token that
 * expires on its own.
 *
 * @param {Io} io
 * @param {string} host
 * @param {string} token
 * @returns {Promise<RevokeOutcome>}
 */
export async function revoke(io, host, token) {
  try {
    const response = await io.fetch(`${host}/api/auth/token`, {
      method: "DELETE",
      headers: { authorization: `Bearer ${token}` },
      signal: AbortSignal.timeout(TIMEOUT_MS)
    });
    if (response.ok) return "revoked";
    // The deployment no longer accepts it, which was the goal.
    if (response.status === 401 || response.status === 403) return "already_invalid";
    if (response.status === 404 || response.status === 405) return "unsupported";
    return "failed";
  } catch {
    return "failed";
  }
}
