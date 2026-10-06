/**
 * `oxyc init-ci`'s registration half: make the deployment trust the workflow
 * the other half just wrote.
 *
 * A trust policy hangs off a service account and says "a run of THIS workflow
 * file, in THIS repository and environment, may act as this account". Two
 * rows, then: the account (`deployer`, unless one was named) and the policy,
 * granted nothing but publishing the one app.
 *
 * NEVER FATAL. Whoever runs `init-ci` may not be an org admin, may be logged in
 * with a token (which cannot create either row — both routes want a browser
 * session), may be offline, or may be talking to a deployment that has neither
 * route. In every one of those the workflow file is still correct and still
 * worth writing, so this returns `manual` with the reason and whatever ids it
 * did learn, and the caller prints the steps with those ids filled in.
 *
 * IDEMPOTENT. Re-running `init-ci --force` finds the account and the policy it
 * made last time rather than stacking a second policy beside the first.
 */

import { spawnSync } from "node:child_process";
import { type ApiResponse, parseJson, request } from "../api/request.js";
import type { Context } from "../context/resolve.js";
import { CliError } from "../util/errors.js";
import { resolveOrgId } from "./assume.js";

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
  | ({ kind: "manual"; reason: string } & KnownIds);

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

async function findAppId(call: Call, orgId: string, plan: RegistrationPlan): Promise<string> {
  const response = await call("GET", `/api/orgs/${orgId}/apps`);
  if (!ok(response)) throw refusedBecause(response, "listing the org's apps", plan.org);
  const app = list<{ id: string; slug: string }>(response, "apps").find((a) => a.slug === plan.app);
  if (!app) {
    throw new Stopped(
      `${plan.org}/${plan.app} is not registered on this deployment yet — publish it once, then re-run \`oxyc init-ci --force\``
    );
  }
  return app.id;
}

async function findOrCreateAccount(
  call: Call,
  orgId: string,
  plan: RegistrationPlan
): Promise<{ account: ServiceAccount; created: boolean }> {
  const path = `/api/orgs/${orgId}/service-accounts`;
  const find = async (): Promise<ServiceAccount | undefined> => {
    const response = await call("GET", path);
    if (!ok(response)) throw refusedBecause(response, "listing service accounts", plan.accountOrg);
    return list<ServiceAccount>(response, "service_accounts").find(
      (a) => a.name === plan.accountName
    );
  };

  const existing = await find();
  if (existing) return { account: existing, created: false };
  if (plan.accountNamed) {
    throw new Stopped(
      `there is no service account ${plan.accountOrg}/${plan.accountName} — create it, or drop --service-account to have \`${DEFAULT_SERVICE_ACCOUNT}\` created`
    );
  }

  const created = await call("POST", path, {
    name: plan.accountName,
    description: "Publishes from GitHub Actions. Created by `oxyc init-ci`.",
    org_role: "member"
  });
  if (ok(created)) {
    return { account: parseJson(created.body) as ServiceAccount, created: true };
  }
  // 409 `name_taken`: someone made it between the list and the create.
  if (created.status === 409) {
    const raced = await find();
    if (raced) return { account: raced, created: false };
  }
  throw refusedBecause(created, "creating the service account", plan.accountOrg);
}

/** `repos/<owner>/<repo>` ids from `gh`, for a repository the server cannot see. */
export function repositoryIds(
  repository: string
): { repository_id: number; repository_owner_id: number } | undefined {
  try {
    const result = spawnSync("gh", ["api", `repos/${repository}`, "--jq", "[.id, .owner.id]"], {
      encoding: "utf8"
    });
    if (result.status !== 0) return undefined;
    const [id, ownerId] = JSON.parse(result.stdout) as [unknown, unknown];
    if (typeof id !== "number" || typeof ownerId !== "number") return undefined;
    return { repository_id: id, repository_owner_id: ownerId };
  } catch {
    return undefined;
  }
}

function samePolicy(policy: TrustPolicy, plan: RegistrationPlan): boolean {
  return (
    !policy.disabled_at &&
    policy.repository.toLowerCase() === plan.repository?.toLowerCase() &&
    policy.workflow_path === plan.workflowPath &&
    (policy.environment ?? "").toLowerCase() === plan.environment.toLowerCase()
  );
}

async function findOrCreatePolicy(
  call: Call,
  path: string,
  plan: RegistrationPlan,
  appId: string,
  lookUpIds: typeof repositoryIds
): Promise<boolean> {
  const existing = await call("GET", path);
  if (!ok(existing)) throw refusedBecause(existing, "listing trust policies", plan.accountOrg);
  if (list<TrustPolicy>(existing, "trust_policies").some((p) => samePolicy(p, plan))) return false;

  const body = {
    repository: plan.repository,
    workflow_path: plan.workflowPath,
    environment: plan.environment,
    // The narrowest grant that does the job: publish this app, nothing else.
    grants: [{ kind: "app_publish", app_id: appId }]
  };
  let created = await call("POST", path, body);
  const code = (parseJson(created.body) as { code?: string } | undefined)?.code;
  if (created.status === 422 && code === "repository_unresolved") {
    // A private repository with no GitHub App installation on the org: the
    // server cannot look its ids up. This machine's `gh` usually can.
    const ids = lookUpIds(plan.repository ?? "");
    if (!ids) {
      throw new Stopped(
        `the deployment could not resolve ${plan.repository} on GitHub (a private repository it has no installation for), and \`gh\` is not available here to supply its ids`
      );
    }
    created = await call("POST", path, { ...body, ...ids });
  }
  if (created.status === 404) {
    // The list above answered, so the route exists: this 404 is the grant's
    // app, which the server looks for in the account's org.
    throw new Stopped(
      `creating the trust policy answered 404 — the deployment found no app ${plan.org}/${plan.app} in ${plan.accountOrg} to grant publishing, or the service account is gone`
    );
  }
  if (!ok(created)) throw refusedBecause(created, "creating the trust policy", plan.accountOrg);
  return true;
}

async function attempt(
  ctx: Context,
  plan: RegistrationPlan,
  known: KnownIds,
  lookUpIds: typeof repositoryIds
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
  const accountOrgId = known.orgId;
  const { account, created: createdAccount } = await findOrCreateAccount(call, accountOrgId, plan);
  known.accountId = account.id;

  const createdPolicy = await findOrCreatePolicy(
    call,
    `/api/orgs/${accountOrgId}/service-accounts/${account.id}/trust-policies`,
    plan,
    known.appId,
    lookUpIds
  );
  return { kind: "registered", appId: known.appId, createdAccount, createdPolicy, ...known };
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
  lookUpIds: typeof repositoryIds = repositoryIds
): Promise<Registration> {
  const known: KnownIds = {};
  try {
    return await attempt(ctx, plan, known, lookUpIds);
  } catch (cause) {
    const reason =
      cause instanceof Stopped || cause instanceof CliError
        ? cause.message
        : `unexpected: ${(cause as Error)?.message ?? String(cause)}`;
    return { kind: "manual", reason, ...known };
  }
}
