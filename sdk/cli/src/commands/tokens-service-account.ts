/**
 * `oxyc tokens create --org-service-account <org>/<account>` — create (or reuse)
 * an organization's service account and mint its token (`oxy_sat_…`), approved
 * once in the browser by an owner or admin of that organization.
 *
 * WHAT IT IS FOR: a credential that belongs to the organization rather than to
 * a person — the kind a custom app's function holds as a secret to call the
 * organization's own admin routes. The organization's API access page does the
 * same two things in two clicks; this asks for both from the terminal.
 *
 * MINTING STAYS BROWSER-SESSION-ONLY. The server refuses any token that asks to
 * create an account or mint a token (`403 session_required`), so a leaked one
 * can never mint its successor. This command holds no credential for the
 * mint: it opens the approval page, as `--agent` and `--sandbox-agent` do (or,
 * with `--no-browser`, asks by device code), and an owner or admin of the
 * organization reads what will be created and approves it under their own
 * session. What comes back is checked before it goes anywhere.
 *
 * WHERE THE SECRET GOES, and only there:
 *
 * - without `--set-app-secret`, to stdout, once, as the only thing on stdout;
 * - with `--set-app-secret <org>/<app> --key <KEY>`, into that app's secret for
 *   `--app-env` (production unless `staging`), through the app's secrets route,
 *   on this machine's own `oxyc login` (an owner or app admin). It is then
 *   NEVER PRINTED — not on stdout, not on stderr, not in an error.
 *
 * Either way it is never written to the credentials file. When anything after
 * the mint fails (the check, or storing the secret), the token is revoked with
 * itself, so no secret outlives a failed run unseen.
 *
 * WHAT IT NEVER DOES: change an existing account. One of that name at another
 * role is refused on the approval page; its role is changed on the API access
 * page, where the effect on its other tokens is spelled out.
 */

import { hostname as osHostname } from "node:os";
import { parseJson, request } from "../api/request.js";
import { UUID_RE } from "../apps/resolve.js";
import { deviceRound } from "../auth/device.js";
import {
  type BrowserPurpose,
  browserRound,
  type CodeRefused,
  exchangeCliCode,
  type LoginOptions,
  type Minted
} from "../auth/login.js";
import { createPkce } from "../auth/pkce.js";
import {
  describeExpiry,
  introspectToken,
  revokeCallingToken,
  type Token
} from "../auth/token-api.js";
import {
  isWellFormedServiceAccountToken,
  SERVICE_ACCOUNT_TOKEN_PREFIX
} from "../auth/token-kind.js";
import type { Context } from "../context/resolve.js";
import * as log from "../ui/log.js";
import { out } from "../ui/tty.js";
import { CliError, ExitCode, exitCodeForStatus, usageError } from "../util/errors.js";
import { printable } from "../util/printable.js";
import { type AppRow, appLabel, openApp } from "./apps-client.js";
import { discard, discardHint, parseName } from "./tokens-sandbox.js";
import { type Mismatch, notAsked, unconfirmed } from "./tokens-sandbox-verify.js";

/** The server's limits on a service-account token's lifetime (`access::expiry`). */
export const SERVICE_ACCOUNT_MINT = { defaultDays: 90, minDays: 1, maxDays: 3650 } as const;

export type AccountRole = "member" | "admin";

/** The environments whose secrets this may write. A sandbox's are `oxyc env secret`'s. */
const APP_ENVS = ["production", "staging"] as const;
export type SecretEnvironment = (typeof APP_ENVS)[number];

export interface ServiceAccountMintFlags {
  /**
   * `--org-service-account <org>/<account>`, as typed. Not `--service-account`:
   * that is the global flag naming the account a GitHub Actions run acts as.
   */
  serviceAccount: string;
  role?: string;
  tokenName?: string;
  expiresInDays?: string;
  /** `--set-app-secret <org>/<app>` (or an app id). */
  setAppSecret?: string;
  key?: string;
  appEnv?: string;
}

export interface ServiceAccountMintRequest {
  /** The org as named: a slug or an id. */
  org: string;
  account: string;
  role: AccountRole;
  name: string;
  days: number;
}

export interface AppSecretTarget {
  /** `<org>/<app>` or an app id, as named. */
  app: string;
  key: string;
  environment: SecretEnvironment;
}

export interface ServiceAccountPlan {
  mint: ServiceAccountMintRequest;
  secret?: AppSecretTarget;
}

const MINT_PURPOSE: BrowserPurpose = {
  opening: "approve a service account token",
  retry: "oxyc tokens create --org-service-account",
  done: "Service account token approved ✓",
  waitingFor: "approve the token"
};

const DISCARD_HINT = discardHint("service account tokens");

/**
 * An org slug, or an id. ANCHORED: the value goes into the `/cli-auth` URL,
 * encoded, but a slug that could carry `&` or `/` is no slug anyway.
 */
const ORG_RE = /^[A-Za-z0-9][A-Za-z0-9._-]*$/;
/** The server's limit on a mint's `org`. */
const ORG_MAX = 100;
/** The server's account-name rule (`account_access::clean_account_name`). */
const ACCOUNT_RE = /^[a-z][a-z0-9]*(-[a-z0-9]+)*$/;
const ACCOUNT_MIN = 2;
const ACCOUNT_MAX = 40;

function parseAccountRef(raw: string): { org: string; account: string } {
  const parts = raw.split("/");
  const [org = "", account = ""] = parts;
  if (parts.length !== 2 || !org || !account) {
    throw usageError(
      `--org-service-account ${JSON.stringify(raw)} is not <org>/<account>`,
      "e.g. --org-service-account acme/store-ops-tablets — the org by slug or id, then the account's name"
    );
  }
  if (!ORG_RE.test(org) || org.length > ORG_MAX) {
    throw usageError(`${JSON.stringify(org)} is not an organization's slug or id`);
  }
  if (account.length < ACCOUNT_MIN || account.length > ACCOUNT_MAX || !ACCOUNT_RE.test(account)) {
    throw usageError(
      `${JSON.stringify(account)} is not a service account name`,
      `${ACCOUNT_MIN} to ${ACCOUNT_MAX} lowercase letters and digits, starting with a letter, with single hyphens between`
    );
  }
  return { org, account };
}

function parseRole(raw: string | undefined): AccountRole {
  if (raw === undefined || raw === "member") return "member";
  if (raw === "admin") return "admin";
  if (raw === "owner") {
    throw usageError("a service account cannot be an owner", "--role member or --role admin");
  }
  throw usageError(`--role ${JSON.stringify(raw)} is not member or admin`);
}

function parseDays(raw: string | undefined): number {
  if (raw === undefined) return SERVICE_ACCOUNT_MINT.defaultDays;
  const days = Number(raw);
  const { minDays, maxDays, defaultDays } = SERVICE_ACCOUNT_MINT;
  if (!Number.isInteger(days) || days < minDays || days > maxDays) {
    throw usageError(
      `--expires-in-days ${JSON.stringify(raw)} is not a whole number from ${minDays} to ${maxDays}`,
      `the default is ${defaultDays}; an organization's token policy may cap it lower`
    );
  }
  return days;
}

/** A secret key is one name: the secrets route holds it to its own grammar. */
function parseKey(raw: string | undefined): string {
  const key = raw?.trim() ?? "";
  if (!key) throw usageError("--set-app-secret needs --key <KEY>, the secret's name");
  if (/[\s/]/.test(key)) {
    throw usageError(
      `${JSON.stringify(raw)} is not a secret key`,
      "a key is one name, e.g. TABLETS_API_KEY — no spaces, no slashes"
    );
  }
  return key;
}

function parseAppEnv(raw: string | undefined): SecretEnvironment {
  if (raw === undefined) return "production";
  if ((APP_ENVS as readonly string[]).includes(raw)) return raw as SecretEnvironment;
  throw usageError(
    `--app-env ${JSON.stringify(raw)} is not production or staging`,
    "a sandbox's secrets are set with `oxyc env secret set`"
  );
}

function parseSecretTarget(flags: ServiceAccountMintFlags): AppSecretTarget | undefined {
  if (flags.setAppSecret === undefined) {
    if (flags.key !== undefined || flags.appEnv !== undefined) {
      throw usageError(
        "--key and --app-env are only valid with --set-app-secret",
        "without it the token is printed once on stdout"
      );
    }
    return undefined;
  }
  const app = flags.setAppSecret.trim();
  if (!UUID_RE.test(app)) {
    const [org = "", slug = "", ...rest] = app.split("/");
    if (!org || !slug || rest.length > 0) {
      throw usageError(
        `--set-app-secret ${JSON.stringify(flags.setAppSecret)} is not <org>/<app>`,
        '<app> is "<org-slug>/<app-slug>" or an app id'
      );
    }
  }
  return { app, key: parseKey(flags.key), environment: parseAppEnv(flags.appEnv) };
}

/**
 * Validate what was asked for. EVERY USAGE ERROR BEFORE ANY BROWSER OPENS: a
 * request the server would refuse must not cost the approver a click first.
 */
export function parseServiceAccountMint(
  flags: ServiceAccountMintFlags,
  hostname: string
): ServiceAccountPlan {
  const { org, account } = parseAccountRef(flags.serviceAccount);
  const mint: ServiceAccountMintRequest = {
    org,
    account,
    role: parseRole(flags.role),
    name: parseName(flags.tokenName, `minted by oxyc on ${hostname}`),
    days: parseDays(flags.expiresInDays)
  };
  const secret = parseSecretTarget(flags);
  return secret ? { mint, secret } : { mint };
}

/** The parameters added to `/cli-auth`, already encoded. */
export function serviceAccountMintQuery(ask: ServiceAccountMintRequest): string {
  return [
    "kind=service_account",
    `org=${encodeURIComponent(ask.org)}`,
    `account=${encodeURIComponent(ask.account)}`,
    `role=${ask.role}`,
    `days=${ask.days}`,
    `name=${encodeURIComponent(ask.name)}`
  ].join("&");
}

/**
 * Whether `app` is in the organization the account is. A token of one org held
 * as another org's secret would hand that app's code the first org's reach.
 */
export function sameOrg(org: string, app: Pick<AppRow, "org_id" | "org_slug">): boolean {
  return UUID_RE.test(org) ? org.toLowerCase() === app.org_id.toLowerCase() : org === app.org_slug;
}

// ── Checking what came back ──────────────────────────────────────────────────

const unsupportedServiceAccount = (target: string) =>
  `${target} does not support minting service account tokens from oxyc yet`;

const DAY_MS = 86_400_000;

/** `1 day`, `90 days`. */
const days = (n: number) => `${n} day${n === 1 ? "" : "s"}`;
/** The approval and the exchange are minutes apart, and clocks drift. */
const EXPIRY_SLACK_MS = 15 * 60_000;

type Described = Record<string, unknown>;

/**
 * Why the token is not the one asked for, if it is not. Asked of the token's
 * own description: kind, owner, name, expiry, and one org-wide grant at no
 * more than the asked role. LESS IS NEVER SILENTLY MORE: a grant above the
 * role, a second grant, or no expiry is refused.
 */
export function serviceAccountMismatch(
  target: string,
  ask: ServiceAccountMintRequest,
  token: Described,
  now: number = Date.now()
): Mismatch | undefined {
  const quoted = (value: unknown) => JSON.stringify(value ?? null);
  if (token.kind !== "service_account") {
    return {
      what: unsupportedServiceAccount(target),
      why: `it described the token as kind ${quoted(token.kind)}, not "service_account"`
    };
  }
  const owner = token.owner as { type?: unknown; label?: unknown } | undefined;
  if (owner?.type !== "service_account" || owner.label !== ask.account) {
    return {
      what: notAsked(target),
      why: `it acts as ${quoted(owner?.label)}, and the account ${quoted(ask.account)} was asked for`
    };
  }
  if (token.name !== ask.name) {
    return {
      what: notAsked(target),
      why: `it is named ${quoted(token.name)}, not ${quoted(ask.name)}`
    };
  }
  if (token.all_access !== false) {
    return { what: notAsked(target), why: "it is not described as grant-bound" };
  }
  const grants = Array.isArray(token.grants) ? (token.grants as Described[]) : undefined;
  const grant = grants?.length === 1 ? grants[0] : undefined;
  if (!grant || grant.workspace_id !== null || grant.revoked_at) {
    return {
      what: notAsked(target),
      why: `it carries ${grants ? grants.length : "unreadable"} grants, and one org-wide grant was asked for`
    };
  }
  if (grant.role_ceiling !== ask.role) {
    return {
      what: notAsked(target),
      why: `its grant is at ${quoted(grant.role_ceiling)}, and ${ask.role} was asked for`
    };
  }
  const expires = typeof token.expires_at === "string" ? Date.parse(token.expires_at) : Number.NaN;
  if (Number.isNaN(expires)) {
    return {
      what: unconfirmed(target),
      why: `its expiry ${quoted(token.expires_at)} cannot be read`
    };
  }
  if (expires > now + ask.days * DAY_MS + EXPIRY_SLACK_MS) {
    return {
      what: notAsked(target),
      why: `it expires ${token.expires_at as string}, after the ${days(ask.days)} asked for`
    };
  }
  return undefined;
}

/**
 * Whether the minted credential is the token asked for. The secret's shape
 * first — it is printed or stored, so it is letters and digits or nothing —
 * then the token's own description, read back with the token itself, which is
 * also proof the deployment accepts what it minted.
 */
export async function verifyServiceAccountToken(
  target: string,
  ask: ServiceAccountMintRequest,
  minted: Minted
): Promise<{ problem: Mismatch } | { token: Token }> {
  if (!minted.token.startsWith(SERVICE_ACCOUNT_TOKEN_PREFIX)) {
    return {
      problem: {
        what: notAsked(target),
        why: "the secret it returned is not a service account token (`oxy_sat_…`)"
      }
    };
  }
  if (!isWellFormedServiceAccountToken(minted.token)) {
    return {
      problem: {
        what: notAsked(target),
        why: "the secret it returned is not a whole service account token (`oxy_sat_` and 36 letters or digits)"
      }
    };
  }
  const found = await introspectToken(target, minted.token);
  if (found.kind !== "token") {
    return {
      problem: {
        what: unconfirmed(target),
        why: `its description could not be read back (GET /api/auth/token answered ${found.kind === "unavailable" ? found.status || "nothing" : "no token"})`
      }
    };
  }
  const problem = serviceAccountMismatch(target, ask, found.token as unknown as Described);
  return problem ? { problem } : { token: found.token };
}

// ── The approval ─────────────────────────────────────────────────────────────

/**
 * What a refused exchange means here. The deployment answers `400
 * invalid_code` for a spent or expired code AND for an approved mint it can no
 * longer honour: who may approve is asked again at the exchange.
 */
const MINT_CODE_REFUSED: CodeRefused = {
  message: "the approval code was rejected, and no token was minted",
  detail:
    "a code is single-use and lives five minutes. The deployment answers the same when it can no longer mint what was approved: the approver is no longer an owner or admin of the organization, or the account changed in the meantime.",
  hint: `run \`${MINT_PURPOSE.retry}\` again`
};

const refuse = (target: string, problem: Mismatch, secret?: string) =>
  discard(target, problem, secret, DISCARD_HINT);

async function approvedInBrowser(
  target: string,
  opts: LoginOptions,
  query: string
): Promise<Minted> {
  const pkce = createPkce();
  const callback = await browserRound(target, opts, pkce, { query, purpose: MINT_PURPOSE });
  // A session token in the callback is the approver's browser session, from a
  // deployment with no exchange at all. Never something minted: dropped.
  if (callback.kind === "token") {
    return refuse(target, {
      what: unsupportedServiceAccount(target),
      why: "its /cli-auth page handed back a session token, so it predates revocable CLI tokens"
    });
  }
  const minted = await exchangeCliCode(target, callback.code, pkce.verifier, MINT_CODE_REFUSED);
  if (!minted) {
    return refuse(target, {
      what: unsupportedServiceAccount(target),
      why: "it issued a code and has no exchange route (POST /api/auth/cli/exchange answered 404)"
    });
  }
  return minted;
}

async function approvedByDeviceCode(
  target: string,
  opts: LoginOptions,
  query: string
): Promise<Minted> {
  const minted = await deviceRound(target, opts, createPkce(), {
    query,
    purpose: MINT_PURPOSE,
    refused: MINT_CODE_REFUSED
  });
  if (!minted) {
    return refuse(target, {
      what: unsupportedServiceAccount(target),
      why: "it has no device-code approval (POST /api/auth/cli/device answered 404)"
    });
  }
  return minted;
}

// ── Storing it as an app's secret ────────────────────────────────────────────

/** The app the secret goes to, resolved on this machine's login before anything is minted. */
interface ResolvedSecretTarget extends AppSecretTarget {
  row: AppRow;
  conn: Awaited<ReturnType<typeof openApp>>["conn"];
}

async function resolveSecretTarget(
  ctx: Context,
  mint: ServiceAccountMintRequest,
  secret: AppSecretTarget
): Promise<ResolvedSecretTarget> {
  const { conn, row } = await openApp(ctx, secret.app);
  if (!sameOrg(mint.org, row)) {
    throw usageError(
      `${appLabel(row)} is not in the organization ${mint.org}`,
      "a service account's token is stored only in an app of the account's own organization"
    );
  }
  return { ...secret, row, conn };
}

/** `text`, with every occurrence of `secret` replaced. What reaches an error goes through it. */
function redacted(text: string, secret: string): string {
  return text.split(secret).join("[redacted]");
}

/**
 * Upsert the app's secret: `201` created, `204` rotated. On this machine's own
 * login, never on the minted token. The value goes into the request body and
 * nowhere else; a refusal is reported without it.
 */
async function storeSecret(
  target: ResolvedSecretTarget,
  value: string
): Promise<"created" | "rotated"> {
  const response = await request({
    target: target.conn.target,
    path: `/api/customer-apps/${encodeURIComponent(target.row.id)}/secrets`,
    method: "POST",
    body: JSON.stringify({ key: target.key, value, environment: target.environment }),
    bearer: target.conn.bearer,
    headers: target.conn.headers
  });
  if (response.status === 201) return "created";
  if (response.status === 204) return "rotated";
  const body = parseJson(response.body) as { error?: unknown; code?: unknown } | undefined;
  const said = typeof body?.error === "string" ? `: ${body.error}` : "";
  throw new CliError(
    `storing ${target.key} on ${appLabel(target.row)} (${target.environment}) failed with ${response.status}${printable(redacted(said, value))}`,
    {
      code: exitCodeForStatus(response.status),
      hint: "setting an app's secret needs an owner or app admin `oxyc login` for this deployment"
    }
  );
}

/**
 * A minted token that could not be stored: nothing holds it now, so it is
 * revoked with itself, and the store's own error is raised — its exit code
 * kept, the value never in it.
 */
async function unstored(
  target: string,
  secret: string,
  tokenId: string,
  error: unknown
): Promise<CliError> {
  const outcome = await revokeCallingToken(target, secret);
  const ended =
    outcome === "revoked" || outcome === "already_invalid"
      ? "The token that was minted for it is revoked, and nothing was kept."
      : `Nothing was kept on this machine, but revoking the token minted for it was not confirmed — revoke token ${printable(tokenId)} on the organization's API access page.`;
  if (error instanceof CliError) {
    return new CliError(redacted(error.message, secret), {
      code: error.code,
      detail: ended,
      hint: error.hint
    });
  }
  return new CliError("storing the token as the app's secret failed", {
    code: ExitCode.UNAVAILABLE,
    detail: ended
  });
}

// ── The command ──────────────────────────────────────────────────────────────

/**
 * Mint a service account's token, and print it once or store it as an app's
 * secret.
 *
 * EVERY STRING THIS COMMAND PRINTS THAT A DEPLOYMENT SUPPLIED goes through
 * `printable`.
 */
export async function runTokensCreateServiceAccount(
  ctx: Context,
  flags: ServiceAccountMintFlags,
  opts: LoginOptions = {}
): Promise<void> {
  const hostname = opts.hostname ?? osHostname();
  const plan = parseServiceAccountMint(flags, hostname);
  const { mint } = plan;
  const target = ctx.target();

  // The app first, on this machine's login: a typo, a missing login or an app
  // of another org is said before anyone is asked to approve anything.
  const store = plan.secret ? await resolveSecretTarget(ctx, mint, plan.secret) : undefined;

  log.info(
    `Asking ${target} for a token of the service account ${mint.account} in ${mint.org} ` +
      `(${mint.role}, ${days(mint.days)})` +
      `${store ? `, to store as ${store.key} on ${appLabel(store.row)} (${store.environment})` : ""}. ` +
      `An owner or admin of ${mint.org} approves it once ${opts.noBrowser ? "from any device" : "in the browser"}.`
  );
  const round = { ...opts, hostname };
  const query = serviceAccountMintQuery(mint);
  const minted = opts.noBrowser
    ? await approvedByDeviceCode(target, round, query)
    : await approvedInBrowser(target, round, query);

  // BEFORE THE SECRET GOES ANYWHERE.
  const verdict = await verifyServiceAccountToken(target, mint, minted);
  if ("problem" in verdict) return refuse(target, verdict.problem, minted.token);
  const { token } = verdict;

  if (store) {
    let outcome: "created" | "rotated";
    try {
      outcome = await storeSecret(store, minted.token);
    } catch (error) {
      throw await unstored(target, minted.token, token.id, error);
    }
    process.stderr.write(
      `${out.green(
        `Stored a new token of ${printable(mint.account)} as ${store.key} on ${appLabel(store.row)} (${store.environment}) — ${outcome === "created" ? "created" : "rotated: the old value is replaced"}.`
      )}\n`
    );
    log.info("the token was not printed and is not kept on this machine");
    log.info(
      "the app's functions read it on their next run; the token it replaced is still valid until revoked on the organization's API access page"
    );
  } else {
    // STDOUT IS THE SECRET AND NOTHING ELSE.
    process.stdout.write(`${minted.token}\n`);
    process.stderr.write(
      `${out.green(`Minted a token of the service account ${printable(mint.account)} in ${printable(mint.org)}.`)}\n`
    );
    log.info("shown once and not stored on this machine");
  }
  log.info(
    `it reaches all of ${printable(mint.org)} as ${mint.role}, and expires ${printable(describeExpiry(token.expires_at))}`
  );
  if (mint.role === "admin") {
    // What an approver handed to code, in plain words: every org-admin route a token may call.
    log.info("as an admin it can also change members' roles, remove members and send invitations");
  }
  log.info(
    `token id ${printable(token.id)} — revoke it on the organization's API access page, or with the token itself: OXY_TOKEN=… oxyc tokens revoke --current`
  );
}
