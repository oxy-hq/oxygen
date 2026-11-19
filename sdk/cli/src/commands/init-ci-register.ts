/**
 * `oxyc init-ci`'s registration half: make the deployment trust the workflow
 * the other half just wrote.
 *
 * A trust policy hangs off a service account and says "a run of THIS workflow
 * file, in THIS repository and environment, may act as this account". Two
 * rows, then: the account (`deployer`, unless one was named) and the policy,
 * granted nothing but publishing the one app.
 *
 * ONE BROWSER APPROVAL, NO TOKEN. Creating either row is browser-session-only
 * on the server, and `oxyc login` caches a token, so this never tries the
 * create routes. It reads what is already there with the stored login (a token
 * may read both), and when the account or the policy is missing it asks an
 * owner or admin of the org to approve both at once: the `/cli-auth` approval
 * `oxyc` mints its tokens with (or a device code, `--no-browser`), for a
 * `trust_policy`. The deployment writes them under the approver's session and
 * answers with what it wrote — the account's id goes into the workflow — and
 * nothing is minted. The page shows the org, the account (new or existing),
 * the app, and the repository, workflow and environment it will trust.
 *
 * NEVER FATAL. Whoever runs `init-ci` may be offline, may be on a CI runner
 * where nobody can approve, may have the approval declined, or may be talking
 * to a deployment without the routes. In every one of those the workflow file
 * is still correct and still worth writing, so this returns `manual` with the
 * reason and whatever ids it did learn, and the caller prints the steps with
 * those ids filled in.
 *
 * IDEMPOTENT. Re-running `init-ci --force` finds the account and the policy it
 * made last time and asks for nothing; the deployment reuses both as well.
 */

import { type ApiResponse, parseJson, request } from "../api/request.js";
import type { Approval } from "../auth/approval.js";
import { deviceRound, deviceUrl } from "../auth/device.js";
import {
  type BrowserPurpose,
  browserRound,
  type CodeRefused,
  exchangeCliApproval,
  type LoginOptions,
  type MintBody
} from "../auth/login.js";
import { createPkce } from "../auth/pkce.js";
import type { Context } from "../context/resolve.js";
import * as log from "../ui/log.js";
import { CliError } from "../util/errors.js";
import { printable } from "../util/printable.js";
import { resolveOrgId } from "./assume.js";
import { collectApproval, loadApproval, pendingKey, startApproval } from "./init-ci-pending.js";

/** What `init-ci` creates when no account is named. */
export const DEFAULT_SERVICE_ACCOUNT = "deployer";

/**
 * The server's rule for a service account's name: lowercase words joined by
 * single hyphens, starting with a letter, 2–40 characters.
 *
 * Checked here so a name the server would refuse with a 400 is a usage error
 * before anything is written or created — and because the name is written
 * unquoted into the workflow, where the rule doubles as the allowlist.
 */
export function isValidServiceAccountName(name: string): boolean {
  return name.length >= 2 && name.length <= 40 && /^[a-z][a-z0-9]*(-[a-z0-9]+)*$/.test(name);
}

export interface RegistrationPlan {
  /** The app's org and slug. */
  org: string;
  app: string;
  /** The service account's org — the app's, unless `--service-account` named another. */
  accountOrg: string;
  accountName: string;
  /** `--service-account` was passed: use it, and never create it. */
  accountNamed: boolean;
  /** `owner/repo`, from the checkout's `origin`. */
  repository?: string;
  workflowPath: string;
  environment: string;
}

/** Ids learned along the way — filled in on the manual steps when it stops short. */
export interface KnownIds {
  orgId?: string;
  appId?: string;
  accountId?: string;
}

export type Registration =
  | ({
      kind: "registered";
      appId: string;
      createdAccount: boolean;
      createdPolicy: boolean;
    } & KnownIds)
  | ({ kind: "manual"; reason: string } & KnownIds)
  /** `--no-wait`: an approval was asked for, and nobody has decided it yet. */
  | ({ kind: "pending"; url: string; userCode: string; expiresAt: string } & KnownIds);

/** A stop that is an answer, not a bug: its message is the reason shown. */
class Stopped extends Error {}

interface ServiceAccount {
  id: string;
  name: string;
}

interface TrustPolicy {
  id: string;
  repository: string;
  workflow_path: string;
  environment: string | null;
  disabled_at?: string | null;
  grants?: { kind?: string; app_id?: string | null }[];
}

/**
 * How to ask an org admin to approve, when something is missing. `route` is
 * decided only then — a run that finds everything in place asks nobody — and
 * may throw where nobody can approve (a CI runner); that is a `manual` reason.
 * `login` carries what tests inject (the browser opener, the hostname, sleep).
 */
export interface ApprovalOptions {
  route: () => Approval;
  login?: LoginOptions;
  /**
   * `--no-wait`: ask, say where, and return `pending` rather than waiting; a
   * re-run with it checks once. Implies the device code.
   */
  noWait?: boolean;
}

const TIMEOUT_MS = 30_000;

function ok(response: ApiResponse): boolean {
  return response.status >= 200 && response.status < 300;
}

/** Why a route refused, in the terms of what the caller can do about it. */
function refusedBecause(response: ApiResponse, what: string, org: string): Stopped {
  const body = parseJson(response.body) as { error?: string; code?: string } | undefined;
  if (response.status === 403 && body?.code === "session_required") {
    return new Stopped(
      `${what} needs a browser session, and this login is a token — it has to be done in the web app`
    );
  }
  if (response.status === 401 || response.status === 403) {
    return new Stopped(`${what} was refused (${response.status}) — it takes an admin of ${org}`);
  }
  if (response.status === 404) {
    return new Stopped(
      `${what} answered 404 — this deployment predates trusted access, or this login cannot see ${org}`
    );
  }
  const said = body?.error ? `: ${body.error}` : "";
  return new Stopped(`${what} failed (${response.status})${said}`);
}

/** One call with the stored bearer. `init-ci` never mints a credential. */
function caller(target: string, bearer: string) {
  return (method: string, path: string, body?: unknown): Promise<ApiResponse> =>
    request({
      target,
      path,
      method,
      bearer,
      body: body === undefined ? undefined : JSON.stringify(body),
      timeoutMs: TIMEOUT_MS
    });
}
type Call = ReturnType<typeof caller>;

function list<T>(response: ApiResponse, key: string): T[] {
  const body = parseJson(response.body);
  if (Array.isArray(body)) return body as T[];
  const nested = (body as Record<string, unknown> | undefined)?.[key];
  return Array.isArray(nested) ? (nested as T[]) : [];
}

/**
 * The app's id, as this login can see it — or `undefined` when it may not list
 * the org's apps (not an org admin). Not knowing is no reason to stop: the
 * approval names the app by slug, and the approver's deployment resolves it.
 */
async function findAppId(
  call: Call,
  orgId: string,
  plan: RegistrationPlan
): Promise<string | undefined> {
  const response = await call("GET", `/api/orgs/${orgId}/apps`);
  if (response.status === 401 || response.status === 403) return undefined;
  if (!ok(response)) throw refusedBecause(response, "listing the org's apps", plan.org);
  const app = list<{ id: string; slug: string }>(response, "apps").find((a) => a.slug === plan.app);
  if (!app) {
    throw new Stopped(
      `${plan.org}/${plan.app} is not registered on this deployment yet — publish it once, then re-run \`oxyc init-ci --force\``
    );
  }
  return app.id;
}

/**
 * The account of the plan's name, as this login can see it: the account,
 * `null` when the org has none of that name, `undefined` when this login may
 * not list them (not an org admin). Not knowing is no reason to stop: the
 * approver may be someone who can.
 */
async function accountIfVisible(
  call: Call,
  orgId: string,
  plan: RegistrationPlan
): Promise<ServiceAccount | null | undefined> {
  const response = await call("GET", `/api/orgs/${orgId}/service-accounts`);
  if (response.status === 401 || response.status === 403) return undefined;
  if (!ok(response)) throw refusedBecause(response, "listing service accounts", plan.accountOrg);
  return (
    list<ServiceAccount>(response, "service_accounts").find((a) => a.name === plan.accountName) ??
    null
  );
}

/** Whether `policy` already trusts this run to publish this app (by id). */
function samePolicy(policy: TrustPolicy, plan: RegistrationPlan, appId: string): boolean {
  return (
    !policy.disabled_at &&
    policy.repository.toLowerCase() === plan.repository?.toLowerCase() &&
    policy.workflow_path === plan.workflowPath &&
    (policy.environment ?? "").toLowerCase() === plan.environment.toLowerCase() &&
    (policy.grants ?? []).some((g) => g.kind === "app_publish" && g.app_id === appId)
  );
}

/** Whether the account already has the policy, as far as this login can see. */
async function policyInPlace(
  call: Call,
  path: string,
  plan: RegistrationPlan,
  appId: string
): Promise<boolean> {
  const existing = await call("GET", path);
  if (!ok(existing)) return false;
  return list<TrustPolicy>(existing, "trust_policies").some((p) => samePolicy(p, plan, appId));
}

const APPROVAL_PURPOSE: BrowserPurpose = {
  opening: "approve the service account and trust policy",
  retry: "oxyc init-ci --force",
  done: "Trust policy approved ✓",
  waitingFor: "approve the trust policy"
};

/**
 * A refused exchange: one answer for a spent or expired code AND for an
 * approval the deployment can no longer carry out — the approver is no longer
 * an owner or admin of the org, or the account changed in the meantime.
 */
const APPROVAL_REFUSED: CodeRefused = {
  message: "the approval code was rejected, and nothing was registered",
  detail:
    "a code is single-use and lives five minutes. The deployment answers the same when it can no longer do what was approved: the approver is no longer an owner or admin of the organization, or the account changed in the meantime.",
  hint: `run \`${APPROVAL_PURPOSE.retry}\` again`
};

/** What the approval asks for, as `/cli-auth` parameters, already encoded. */
export function trustPolicyQuery(ask: {
  orgId: string;
  accountName: string;
  createAccount: boolean;
  /** The app's id, or its slug. */
  app: string;
  repository: string;
  workflowPath: string;
  environment: string;
}): string {
  const e = encodeURIComponent;
  return [
    "kind=trust_policy",
    `org=${e(ask.orgId)}`,
    `account=${e(ask.accountName)}`,
    ...(ask.createAccount ? ["create=1"] : []),
    `app=${e(ask.app)}`,
    `repository=${e(ask.repository)}`,
    `workflow=${e(ask.workflowPath)}`,
    `environment=${e(ask.environment)}`
  ].join("&");
}

/** What the deployment wrote, read off its answer. */
interface Written {
  accountId: string;
  appId: string;
  createdAccount: boolean;
  createdPolicy: boolean;
}

/**
 * The approval's answer, checked against the plan: the account it names, the
 * app (by slug, and by id when this login knew it) and a policy trusting
 * exactly this run to publish exactly that app. Anything else is a refusal —
 * the id that goes into the workflow must be the right one.
 */
export function writtenFrom(
  body: MintBody,
  plan: RegistrationPlan,
  knownAppId: string | undefined
): Written {
  const registration = body.registration as
    | {
        service_account?: { id?: unknown; name?: unknown };
        trust_policy?: TrustPolicy;
        app?: { id?: unknown; slug?: unknown };
        created_account?: unknown;
        created_policy?: unknown;
      }
    | undefined;
  const account = registration?.service_account;
  const policy = registration?.trust_policy;
  const app = registration?.app;
  if (!registration || typeof account?.id !== "string" || !policy || typeof app?.id !== "string") {
    throw new Stopped("the approval came back without the account, app and policy it wrote");
  }
  if (app.slug !== plan.app || (knownAppId !== undefined && app.id !== knownAppId)) {
    throw new Stopped(
      `the deployment registered publishing ${printable(JSON.stringify(app.slug))}, not ${plan.app}`
    );
  }
  const appId = app.id;
  if (account.name !== plan.accountName) {
    throw new Stopped(
      `the deployment registered the account ${printable(JSON.stringify(account.name))}, not ${plan.accountName}`
    );
  }
  if (!samePolicy(policy, plan, appId)) {
    throw new Stopped(
      `the deployment registered a policy that is not the one asked for (${printable(policy.repository)} ${printable(policy.workflow_path)})`
    );
  }
  return {
    accountId: account.id,
    appId,
    createdAccount: registration.created_account === true,
    createdPolicy: registration.created_policy === true
  };
}

/** One approval, by browser or device code, collected as the deployment's answer. */
async function approved(
  target: string,
  route: Approval,
  login: LoginOptions,
  query: string
): Promise<MintBody> {
  const opts = { ...login, noBrowser: route.noBrowser, approver: route.approver };
  if (route.noBrowser) {
    const body = await deviceRound(target, opts, createPkce(), {
      query,
      purpose: APPROVAL_PURPOSE,
      refused: APPROVAL_REFUSED,
      collect: (answer: MintBody) => answer
    });
    if (!body) {
      throw new Stopped(
        "this deployment cannot approve anything by device code yet (POST /api/auth/cli/device answered 404)"
      );
    }
    return body;
  }
  const pkce = createPkce();
  const callback = await browserRound(target, opts, pkce, { query, purpose: APPROVAL_PURPOSE });
  if (callback.kind === "token") {
    throw new Stopped(
      "this deployment's approval page handed back a session token: it predates approving a trust policy from oxyc"
    );
  }
  const body = await exchangeCliApproval(target, callback.code, pkce.verifier, APPROVAL_REFUSED);
  if (!body) {
    throw new Stopped(
      "this deployment has no approval exchange (POST /api/auth/cli/exchange answered 404)"
    );
  }
  return body;
}

async function attempt(
  ctx: Context,
  plan: RegistrationPlan,
  known: KnownIds,
  approval: ApprovalOptions | undefined
): Promise<Registration> {
  const bearer = ctx.storedBearer();
  if (!bearer) throw new Stopped(`not logged in to ${ctx.target()}`);
  if (!plan.repository) {
    throw new Stopped("this checkout has no `origin` remote on GitHub to trust");
  }
  const call = caller(ctx.target(), bearer);

  if (plan.accountOrg !== plan.org) {
    // The server checks an `app_publish` grant's app against the ACCOUNT's
    // org, and answers 404 for another org's — which would read here as a
    // deployment without trusted access.
    throw new Stopped(
      `${plan.accountOrg}/${plan.accountName} cannot be granted publishing ${plan.org}/${plan.app}: a service account's grants are in its own organization`
    );
  }

  known.orgId = await resolveOrgId(ctx, plan.org);
  known.appId = await findAppId(call, known.orgId, plan);
  const appId = known.appId;

  // What is already there, as far as this login can see. Both in place: done,
  // and nobody is asked anything.
  const account = await accountIfVisible(call, known.orgId, plan);
  if (account) {
    known.accountId = account.id;
    const path = `/api/orgs/${known.orgId}/service-accounts/${account.id}/trust-policies`;
    if (appId && (await policyInPlace(call, path, plan, appId))) {
      return { kind: "registered", appId, createdAccount: false, createdPolicy: false, ...known };
    }
  } else if (account === null && plan.accountNamed) {
    throw new Stopped(
      `there is no service account ${plan.accountOrg}/${plan.accountName} — create it, or drop --service-account to have \`${DEFAULT_SERVICE_ACCOUNT}\` created`
    );
  }

  if (!approval) {
    throw new Stopped(
      "creating the service account and its trust policy needs an org admin's approval in a browser"
    );
  }
  const target = ctx.target();
  const key = pendingKey(target, plan);
  const login = approval.login ?? {};
  const wording = { refused: APPROVAL_REFUSED, purpose: APPROVAL_PURPOSE };

  // An approval an earlier `--no-wait` asked for: collect it rather than ask again.
  const waiting = loadApproval(key);
  if (waiting) {
    log.info(`Collecting the approval an earlier \`oxyc init-ci --no-wait\` asked for.`);
    const body = await collectApproval(
      target,
      key,
      waiting,
      login,
      wording,
      approval.noWait === true
    );
    if (!body) {
      return {
        kind: "pending",
        url: deviceUrl(target, waiting.user_code),
        userCode: waiting.user_code,
        expiresAt: waiting.expires_at,
        ...known
      };
    }
    return registeredFrom(body, plan, appId, known);
  }

  // Decided now, not before: a run with nothing to create asks nobody.
  const route = approval.route();
  const query = trustPolicyQuery({
    orgId: known.orgId,
    accountName: plan.accountName,
    createAccount: !plan.accountNamed,
    // By slug when this login may not list the org's apps: the approver's
    // deployment resolves it in the org.
    app: appId ?? plan.app,
    repository: plan.repository,
    workflowPath: plan.workflowPath,
    // Never the repository's ids: the approver is shown only its name, so the
    // deployment reads them from GitHub, or refuses a repository it can't see.
    environment: plan.environment
  });
  const remote = approval.noWait || route.noBrowser;
  log.info(
    `Asking an owner or admin of ${plan.accountOrg} to approve the service account ${plan.accountName} and a trust policy for ${plan.repository} — once, ${remote ? "from any device" : "in the browser"}.`
  );
  if (approval.noWait) {
    const opts = { ...login, noBrowser: true, approver: route.approver };
    const started = await startApproval(target, key, opts, { query, purpose: APPROVAL_PURPOSE });
    if (!started) {
      throw new Stopped(
        "this deployment cannot approve anything by device code yet (POST /api/auth/cli/device answered 404)"
      );
    }
    return { kind: "pending", ...started, ...known };
  }
  const body = await approved(target, route, login, query);
  return registeredFrom(body, plan, appId, known);
}

/** The approval's answer, checked, as the registration it is. */
function registeredFrom(
  body: MintBody,
  plan: RegistrationPlan,
  appId: string | undefined,
  known: KnownIds
): Registration {
  const written = writtenFrom(body, plan, appId);
  known.accountId = written.accountId;
  known.appId = written.appId;
  return {
    kind: "registered",
    appId: written.appId,
    createdAccount: written.createdAccount,
    createdPolicy: written.createdPolicy,
    ...known
  };
}

/**
 * The id of the account the plan names, looked up as whoever is logged in.
 *
 * A workflow names its service account BY ID — the deployment takes nothing
 * else — so the id has to be written into the file even when nothing is being
 * registered (`--no-register`) or registration stopped before it reached the
 * account. Read-only: it lists, and creates nothing.
 *
 * Never throws. `undefined` is "could not find out" — not logged in, not
 * allowed to list, offline, or no such account yet — and the caller writes a
 * placeholder and says so rather than the account's name.
 */
export async function lookUpAccountId(
  ctx: Context,
  plan: RegistrationPlan,
  known: KnownIds = {}
): Promise<string | undefined> {
  if (known.accountId) return known.accountId;
  try {
    const bearer = ctx.storedBearer();
    if (!bearer) return undefined;
    const orgId =
      plan.accountOrg === plan.org && known.orgId
        ? known.orgId
        : await resolveOrgId(ctx, plan.accountOrg);
    const response = await caller(ctx.target(), bearer)(
      "GET",
      `/api/orgs/${orgId}/service-accounts`
    );
    if (!ok(response)) return undefined;
    return list<ServiceAccount>(response, "service_accounts").find(
      (a) => a.name === plan.accountName
    )?.id;
  } catch {
    return undefined;
  }
}

/** Register the workflow, or say why not. Never throws. */
export async function registerTrustPolicy(
  ctx: Context,
  plan: RegistrationPlan,
  approval?: ApprovalOptions
): Promise<Registration> {
  const known: KnownIds = {};
  try {
    return await attempt(ctx, plan, known, approval);
  } catch (cause) {
    const reason =
      cause instanceof Stopped || cause instanceof CliError
        ? cause.message
        : `unexpected: ${(cause as Error)?.message ?? String(cause)}`;
    return { kind: "manual", reason, ...known };
  }
}
