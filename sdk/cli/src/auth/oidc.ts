/**
 * GitHub Actions OIDC → an Oxy token, for every command.
 *
 * A job granted `id-token: write` can ask GitHub for a signed statement of
 * what it is — which repository, which workflow file, which environment. The
 * deployment checks that statement against the trust policies of ONE service
 * account — the one the workflow names — and, on a match, hands back an
 * `oxy_ci_` token good for fifteen minutes. No secret is stored anywhere.
 *
 * THE WORKFLOW NAMES THE ACCOUNT, ALWAYS, BY ITS ID (`--service-account`, or
 * `OXY_SERVICE_ACCOUNT`). The deployment never goes looking for an account on
 * a run's behalf: anyone can register a trust policy that names somebody
 * else's repository, so "whichever policy matches" would let a stranger's
 * policy decide what this job becomes. With no account named, no exchange is
 * attempted at all — `oxyc publish` and `oxyc checks run` go straight to the
 * app's registered publisher, and every other command says what is missing.
 *
 * THE ID, NOT `<org-slug>/<name>`. A slug can be changed and, once its org
 * renames or is deleted, taken by anyone — who could then create the same
 * account name under it, and a workflow still saying `acme/deployer` would be
 * naming THEIR account. An id never changes hands. oxyc passes the value
 * through untouched; the deployment refuses anything that is not an id.
 *
 * This is the third and last credential source, after `OXY_TOKEN` and the
 * login cache (`context/resolve.ts`). `oxyc publish` had its own, app-scoped
 * version of this first (`publish/server.ts`, audience `oxy-publish`); that
 * one stays, for a workflow that names no account and for a deployment or an
 * app that has only the older registration.
 */

import { CliError, ExitCode, type ExitCodeValue, exitCodeForStatus } from "../util/errors.js";
import { revokeOnExit } from "./exit-revoke.js";
import type { Grant } from "./token-api.js";

/**
 * The audience of the deployment at `target`: `oxy:<host>`.
 *
 * WORKED OUT FROM THE URL, NEVER ASKED FOR. A GitHub OIDC token is good
 * wherever its audience is accepted, and each deployment accepts the audience
 * of the address it calls itself by. So a token asked for THE HOST IT IS ABOUT
 * TO BE POSTED TO is good at that deployment and nowhere else — whoever is
 * listening on the host. Were the deployment asked which audience to use, a
 * compromised or look-alike one could name another deployment's and be handed
 * a token that one accepts; were there a fallback to a shared audience, any
 * server could trigger it. Neither exists.
 *
 * The host is the URL's own (`URL.host`): lowercased, with `:<port>` only when
 * the port is not the scheme's default. That is exactly what the server
 * derives from its public URL (`deployment_audience`, `crates/auth`), and the
 * cases in `oidc.test.ts` are repeated there so the two cannot drift.
 *
 * Never plain `oxy`: that is what a deployment with no public URL takes, and
 * it is not asked for — GitHub sign-in is not available on such a deployment.
 */
export function oidcAudience(target: string): string {
  let host: string;
  try {
    host = new URL(target).host;
  } catch {
    host = "";
  }
  if (!host) {
    throw new CliError(`cannot sign in with GitHub OIDC at ${JSON.stringify(target)}: not a URL`, {
      code: ExitCode.USAGE,
      hint: "pass --target https://<host>, or an --env that resolves to one"
    });
  }
  return `oxy:${host}`;
}

const OIDC_TIMEOUT_MS = 30_000;

/**
 * The longest a rate-limited exchange waits before its one retry. The server's
 * budget refills at one request a second, so its `Retry-After` is almost always
 * `1`; the cap only matters for a proxy that asks for longer than a CI step
 * should sit idle.
 */
export const RATE_LIMIT_MAX_WAIT_S = 60;

/** `Retry-After` as whole seconds to wait, capped. Missing or unreadable is one. */
export function retryAfterSeconds(header: string | null | undefined): number {
  const seconds = Number.parseInt(header ?? "", 10);
  if (!Number.isFinite(seconds) || seconds < 1) return 1;
  return Math.min(seconds, RATE_LIMIT_MAX_WAIT_S);
}

const sleepFor = (ms: number): Promise<void> => new Promise((done) => setTimeout(done, ms));

/** The two variables GitHub sets in a job granted `id-token: write`. */
export function githubOidcAvailable(env: NodeJS.ProcessEnv = process.env): boolean {
  return Boolean(env.ACTIONS_ID_TOKEN_REQUEST_URL && env.ACTIONS_ID_TOKEN_REQUEST_TOKEN);
}

export type OidcErrorCode =
  // 401 — the token itself was not acceptable.
  | "invalid_token"
  | "wrong_audience"
  | "expired"
  | "replayed"
  // 403 — the token was fine and the run was refused.
  | "pull_request_target"
  | "self_hosted_runner"
  | "missing_environment"
  | "no_matching_policy"
  /**
   * 400 — the request carried no service account ID. From oxyc that means the
   * value it was given is not one: `acme/deployer`, say, instead of the id.
   */
  | "service_account_required"
  /**
   * The run names no service account at all, so oxyc never asked. Ours, not
   * the server's — and the one refusal that is not about this exchange.
   */
  | "no_service_account"
  // 429 — over the per-client budget, still, after one wait and retry.
  | "rate_limited"
  /** 400 with no `code`: the body was unusable — it carried no token. */
  | "malformed"
  /** 404: the deployment predates the exchange. Ours, not the server's. */
  | "unsupported"
  /** GitHub would not mint the id token. Ours, not the server's. */
  | "github"
  | "unknown";

/** A refused exchange, carrying what a caller needs to decide on a fallback. */
export class OidcExchangeError extends CliError {
  /** The HTTP status of the refusal; 0 when no request was made. */
  readonly status: number;
  readonly oidcCode: OidcErrorCode;

  constructor(
    message: string,
    opts: {
      status: number;
      oidcCode: OidcErrorCode;
      code?: ExitCodeValue;
      hint?: string;
      detail?: string;
      remedy?: string;
    }
  ) {
    super(message, opts);
    this.name = "OidcExchangeError";
    this.status = opts.status;
    this.oidcCode = opts.oidcCode;
  }
}

/**
 * Whether a refusal leaves the app-scoped publisher exchange worth trying.
 *
 * Three cases, and only three: the workflow names no service account, so the
 * general exchange was never asked — the app's publisher is then the only
 * door, exactly as it was before trust policies existed; the deployment has no
 * general exchange at all; or it has one and no policy of the named account
 * matched this run — where an app may still carry a publisher registered the
 * older way. Every other code names something wrong with the run itself,
 * which the older exchange would only paper over — the server's own
 * `service_account_required` among them: a workflow that names an account
 * badly has a line to fix, not a second door to try.
 */
export function fallsBackToPublisher(refusal: OidcExchangeError): boolean {
  return (
    refusal.oidcCode === "no_service_account" ||
    refusal.oidcCode === "unsupported" ||
    refusal.oidcCode === "no_matching_policy"
  );
}

/** Where a person finds a service account's id. */
const WHERE_THE_ID_IS =
  "it is shown in the web app under Organization settings → API access → Service accounts → (the account), and `oxyc init-ci` writes it into the workflow.";

/** How a workflow names the account it acts as. */
const NAME_THE_ACCOUNT = `set OXY_SERVICE_ACCOUNT to the service account's ID (or pass --service-account): the account whose trust policy matches this workflow. ${WHERE_THE_ID_IS}`;

/**
 * A job that could mint a GitHub OIDC token and names no account to trade it
 * for. Raised BEFORE any request: nothing is minted, nothing is spent.
 */
export function noServiceAccount(target: string): OidcExchangeError {
  return new OidcExchangeError(`not authenticated for ${target}`, {
    status: 0,
    oidcCode: "no_service_account",
    code: ExitCode.AUTH,
    detail:
      "this job can mint a GitHub OIDC token, but names no service account to act as —\n" +
      "the deployment never picks one on a run's behalf.",
    hint: `${NAME_THE_ACCOUNT}\nOr set OXY_TOKEN from a secret.`
  });
}

/** What the exchange hands back. */
export interface OidcCredential {
  /** `oxy_ci_…` */
  token: string;
  tokenId?: string;
  /** RFC 3339. Fifteen minutes from the exchange. */
  expiresAt?: string;
  /** `<org-slug>/<name>` — the account the run is acting as. */
  serviceAccount?: string;
  grants: Grant[];
}

async function send(url: string, init: RequestInit): Promise<Response> {
  try {
    return await fetch(url, { ...init, signal: AbortSignal.timeout(OIDC_TIMEOUT_MS) });
  } catch (cause) {
    throw new CliError(`${init.method ?? "GET"} ${url} failed: ${(cause as Error).message}`, {
      code: ExitCode.UNAVAILABLE
    });
  }
}

/**
 * Ask GitHub for an id token with the given audience.
 *
 * One call per exchange attempt, always: the server records each token's `jti`
 * and refuses a second use, so a token cannot be minted once and offered to two
 * endpoints.
 */
export async function requestGithubIdToken(
  audience: string,
  env: NodeJS.ProcessEnv = process.env
): Promise<string> {
  const requestUrl = new URL(env.ACTIONS_ID_TOKEN_REQUEST_URL ?? "");
  requestUrl.searchParams.set("audience", audience);
  const minted = await send(requestUrl.toString(), {
    headers: { authorization: `bearer ${env.ACTIONS_ID_TOKEN_REQUEST_TOKEN}` }
  });
  if (!minted.ok) {
    throw new OidcExchangeError(`GitHub refused to mint an OIDC token (${minted.status})`, {
      status: minted.status,
      oidcCode: "github",
      code: ExitCode.AUTH,
      hint: "the job needs `permissions: id-token: write`"
    });
  }
  let value: string | undefined;
  try {
    value = ((await minted.json()) as { value?: string }).value;
  } catch {
    value = undefined;
  }
  if (!value) {
    throw new OidcExchangeError("GitHub returned no OIDC token", {
      status: minted.status,
      oidcCode: "github",
      code: ExitCode.AUTH
    });
  }
  return value;
}

/** What this run looks like to a trust policy, from the variables GitHub sets. */
function runIdentity(env: NodeJS.ProcessEnv): string | undefined {
  const parts = [
    env.GITHUB_REPOSITORY ? `repository  ${env.GITHUB_REPOSITORY}` : "",
    env.GITHUB_WORKFLOW_REF ? `workflow    ${env.GITHUB_WORKFLOW_REF}` : "",
    env.GITHUB_REF ? `ref         ${env.GITHUB_REF}` : "",
    env.GITHUB_EVENT_NAME ? `event       ${env.GITHUB_EVENT_NAME}` : ""
  ].filter(Boolean);
  return parts.length > 0 ? `this run:\n${parts.join("\n")}` : undefined;
}

const KNOWN_CODES: ReadonlySet<string> = new Set([
  "invalid_token",
  "wrong_audience",
  "expired",
  "replayed",
  "pull_request_target",
  "self_hosted_runner",
  "missing_environment",
  "no_matching_policy",
  "service_account_required",
  "rate_limited"
]);

/** The one refusal with no `code`: the server could not use the body at all. */
function malformed(status: number, serverSaid: string | undefined): OidcExchangeError {
  return new OidcExchangeError("the deployment could not read the exchange request", {
    status,
    oidcCode: "malformed",
    code: exitCodeForStatus(status),
    detail: serverSaid,
    hint: "the request oxyc sent did not arrive as it was sent. Re-run the job; if it keeps failing, something between the runner and the deployment is rewriting request bodies."
  });
}

/**
 * `wrong_audience`: the deployment takes another audience than the one oxyc
 * derived from the URL it was pointed at — so it calls itself by another
 * address. The refusal carries the audience it does take, which names that
 * address; plain `oxy` means it has none configured.
 *
 * The fix is always where oxyc is pointed, never which audience it asks for:
 * the audience is the host the token is sent to, and that is the point.
 */
function wrongAudience(
  base: { status: number; oidcCode: OidcErrorCode; code: ExitCodeValue },
  serverSaid: string | undefined,
  theirs: unknown,
  target: string
): OidcExchangeError {
  if (theirs === "oxy") {
    return new OidcExchangeError("GitHub sign-in is not available on this deployment", {
      ...base,
      detail: serverSaid,
      hint: `${target} has no public URL configured (OXY_API_URL), so it has no address of its own to accept a GitHub token for — and oxyc never asks GitHub for a token any deployment would take. Set OXY_TOKEN from a secret, or have the deployment's operator set OXY_API_URL.`
    });
  }
  const host =
    typeof theirs === "string" && theirs.startsWith("oxy:") ? theirs.slice(4) : undefined;
  return new OidcExchangeError(
    "the GitHub OIDC token was minted for the wrong audience: this deployment answers to another address",
    {
      ...base,
      detail: serverSaid,
      hint:
        (host
          ? `this deployment answers to ${host}, and oxyc asks GitHub for a token for the host it is about to send it to (here ${oidcAudience(target).slice(4)}). Point --target (or --env) at https://${host}.`
          : "oxyc asks GitHub for a token for the host it is about to send it to, and this deployment calls itself by another. Point --target (or --env) at the deployment's own address.") +
        "\nIf a wrapper minted the token (for `oxy-publish`, say — good only for an app's registered publisher), let oxyc mint its own."
    }
  );
}

/**
 * One refused exchange, as an error a person can act on.
 *
 * The server's `code` decides the wording — its `error` string is kept as
 * detail, never as the message, because the message is where the fix goes.
 */
export function exchangeError(
  status: number,
  body: { error?: string; code?: string; audience?: unknown } | undefined,
  target: string,
  env: NodeJS.ProcessEnv = process.env
): OidcExchangeError {
  const serverSaid = body?.error?.trim() || undefined;
  if (status === 404) {
    return new OidcExchangeError(`not authenticated for ${target}`, {
      status,
      oidcCode: "unsupported",
      code: ExitCode.AUTH,
      detail:
        "this job can mint a GitHub OIDC token, but the deployment has no token exchange\n" +
        "(POST /api/auth/oidc/exchange answered 404) — it predates trusted access.",
      hint: "set OXY_TOKEN from a secret. `oxyc publish` and `oxyc checks run` still work without one, through the app's registered publisher."
    });
  }
  if (status === 400 && !body?.code) return malformed(status, serverSaid);

  const oidcCode: OidcErrorCode =
    body?.code && KNOWN_CODES.has(body.code) ? (body.code as OidcErrorCode) : "unknown";
  const code = exitCodeForStatus(status);
  const base = { status, oidcCode, code };

  switch (oidcCode) {
    case "invalid_token":
      return new OidcExchangeError("the deployment rejected the GitHub OIDC token", {
        ...base,
        detail: serverSaid,
        hint: "its signature or issuer did not verify. Re-run the job; if it keeps failing, the deployment cannot reach GitHub's signing keys."
      });
    case "wrong_audience":
      return wrongAudience(base, serverSaid, body?.audience, target);
    case "expired":
      return new OidcExchangeError("the GitHub OIDC token expired before it was exchanged", {
        ...base,
        detail: serverSaid,
        hint: "re-run the job. A large clock skew on a self-hosted runner produces the same answer."
      });
    case "replayed":
      return new OidcExchangeError("the GitHub OIDC token had already been used", {
        ...base,
        detail: serverSaid,
        // Not "use the setup-oxyc action": it is not published yet. `oxyc token`
        // does the same exchange and prints the result without revoking it.
        hint: "each token is single-use. Exchange once and share the result: run `oxyc token` in one step and pass what it prints to the later steps as OXY_TOKEN."
      });
    case "pull_request_target":
      return new OidcExchangeError("refused: this run was triggered by `pull_request_target`", {
        ...base,
        detail: serverSaid,
        hint: "that event runs a fork's changes with the base repository's identity, so no trust policy matches it. Trigger on `push`, `pull_request` or `workflow_dispatch`."
      });
    case "self_hosted_runner":
      return new OidcExchangeError("refused: the job ran on a self-hosted runner", {
        ...base,
        detail: serverSaid,
        hint: "the trust policy allows GitHub-hosted runners only. Run the job on a hosted runner, or have an org admin allow self-hosted runners on the policy."
      });
    case "missing_environment":
      // Two ways to get here, and the server's words (the detail) say which:
      // the job names no environment and the policy needs one, or the policy
      // names none while its organization requires every policy to.
      return new OidcExchangeError("refused: a GitHub environment is required and missing", {
        ...base,
        detail: serverSaid,
        hint:
          "if the job declares no `environment:`, add `environment: <name>` to the job, with the name the trust policy was registered under.\n" +
          "If it does, the trust policy itself names no environment while the organization requires one: an org admin sets it on the policy (Organization settings → API access → Service accounts → the account → Trusted access)."
      });
    case "no_matching_policy":
      return new OidcExchangeError(
        "no trust policy of the named service account matches this workflow run",
        {
          ...base,
          detail: [serverSaid, runIdentity(env)].filter(Boolean).join("\n") || undefined,
          hint:
            "check the account named by --service-account / OXY_SERVICE_ACCOUNT, and register a policy on it — in the web app under Organization settings → API access → Service accounts → (the account) → Trusted access, or with `oxyc init-ci` from this repo.\n" +
            "The repository, the workflow file and the job's `environment:` must all match the policy."
        }
      );
    case "service_account_required":
      return new OidcExchangeError(
        "the service account must be named by its ID, and this run did not name one",
        {
          ...base,
          detail: serverSaid,
          hint: `--service-account / OXY_SERVICE_ACCOUNT takes the account's ID (a UUID), never <org-slug>/<name>: a name can be taken over by another organization, an ID cannot.\n${WHERE_THE_ID_IS}`
        }
      );
    case "rate_limited":
      return new OidcExchangeError("the deployment is rate-limiting token exchanges from here", {
        ...base,
        // Retryable, unlike the 4xx it arrives as.
        code: ExitCode.UNAVAILABLE,
        detail: serverSaid,
        hint: "the exchange allows 60 requests a minute per client address, and oxyc already waited once and retried. Re-run the job; if many jobs share one egress address, exchange once per job with `oxyc token` and pass what it prints to the later steps as OXY_TOKEN."
      });
    default:
      return new OidcExchangeError(`the OIDC token exchange failed (${status})`, {
        ...base,
        detail: serverSaid ?? (body ? JSON.stringify(body).slice(0, 2000) : undefined)
      });
  }
}

interface ExchangeResponse {
  token?: string;
  token_id?: string;
  expires_at?: string;
  service_account?: string;
  grants?: Grant[];
}

/** One attempt: a fresh id token for `audience`, posted to the exchange. */
async function attempt(
  url: string,
  env: NodeJS.ProcessEnv,
  serviceAccount: string,
  audience: string
) {
  const idToken = await requestGithubIdToken(audience, env);
  return send(url, {
    method: "POST",
    headers: { "content-type": "application/json", accept: "application/json" },
    body: JSON.stringify({ token: idToken, service_account: serviceAccount })
  });
}

/**
 * Mint a GitHub id token and exchange it. One network round trip each. The
 * token's audience is {@link oidcAudience} of `target`: derived, not fetched.
 *
 * NO ACCOUNT, NO REQUEST: with `serviceAccount` unset this throws
 * {@link noServiceAccount} before GitHub or the deployment is asked for
 * anything. The deployment would answer 400 anyway; not asking keeps the
 * single-use id token for the publisher exchange that may follow.
 *
 * A 429 is waited out ONCE, for the server's `Retry-After` (capped at
 * {@link RATE_LIMIT_MAX_WAIT_S}), and retried with a fresh id token. The
 * budget is checked before the token is read, so the first one was not spent —
 * but a fresh one costs a GitHub call and holds whatever sits in between.
 *
 * Throws `OidcExchangeError` for every refusal, so a caller with a fallback
 * (`oxyc publish`) can read `oidcCode` rather than parse a message.
 */
export async function exchangeOidc(
  target: string,
  opts: {
    serviceAccount?: string;
    env?: NodeJS.ProcessEnv;
    /** For tests. */
    sleep?: (ms: number) => Promise<void>;
  } = {}
): Promise<OidcCredential> {
  const env = opts.env ?? process.env;
  const serviceAccount = opts.serviceAccount?.trim();
  if (!serviceAccount) throw noServiceAccount(target);
  // From the URL the token is about to be posted to — no request, and
  // nothing the deployment says can change it.
  const audience = oidcAudience(target);
  const url = `${target.replace(/\/+$/, "")}/api/auth/oidc/exchange`;
  let response = await attempt(url, env, serviceAccount, audience);
  if (response.status === 429) {
    const wait = retryAfterSeconds(response.headers.get("retry-after"));
    await response.arrayBuffer().catch(() => undefined);
    await (opts.sleep ?? sleepFor)(wait * 1000);
    response = await attempt(url, env, serviceAccount, audience);
  }
  const text = await response.text();
  let body: unknown;
  try {
    body = JSON.parse(text);
  } catch {
    body = undefined;
  }
  if (!response.ok) {
    throw exchangeError(response.status, body as Parameters<typeof exchangeError>[1], target, env);
  }
  const minted = (body ?? {}) as ExchangeResponse;
  if (!minted.token) {
    throw new OidcExchangeError("the OIDC exchange returned no token", {
      status: response.status,
      oidcCode: "unknown",
      code: ExitCode.UNAVAILABLE,
      detail: text.slice(0, 2000) || undefined
    });
  }
  return {
    token: minted.token,
    tokenId: minted.token_id,
    expiresAt: minted.expires_at,
    serviceAccount: minted.service_account,
    grants: Array.isArray(minted.grants) ? minted.grants : []
  };
}

/** `target\nservice-account` → the exchange in flight or done. */
const exchanges = new Map<string, Promise<OidcCredential>>();

/**
 * The exchange, at most once per process for a given target and account.
 *
 * ONCE, because a second exchange would spend a second single-use id token to
 * mint a second fifteen-minute credential for a command that needed one — and
 * a paginated `oxyc api` asks for its bearer on every page. A refusal is
 * memoised too: the answer to "does a policy match this run" does not change
 * between two calls in one process. With no account named it is the refusal
 * that is memoised — no request is ever made.
 *
 * The minted token is queued for revocation on exit; `keepPastExit` takes it
 * back out for the one command whose output is the token.
 */
export function exchangeOidcOnce(target: string, serviceAccount?: string): Promise<OidcCredential> {
  const key = `${target}\n${serviceAccount ?? ""}`;
  let exchange = exchanges.get(key);
  if (!exchange) {
    exchange = exchangeOidc(target, { serviceAccount }).then((credential) => {
      revokeOnExit(target, credential.token);
      return credential;
    });
    // A caller that only ever awaits it later must not trip node's
    // unhandled-rejection handler, which `main.ts` wires to a non-zero exit.
    exchange.catch(() => {});
    exchanges.set(key, exchange);
  }
  return exchange;
}

/** Forget every exchange. For tests, which stand in for separate processes. */
export function resetOidcExchanges(): void {
  exchanges.clear();
}
