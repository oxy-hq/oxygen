/**
 * The verify pass: one read per phase, and nothing that writes.
 *
 * WHAT COUNTS AS LANDED IS THE WHOLE VALUE HERE. "I ran the command" is not
 * evidence — `oxyc publish` can succeed onto a build production never serves,
 * an org can be created with an owner who never accepts the invitation, and a
 * workspace can hold a remote with nothing compiled. So every reading below
 * asks the deployment for the FACT the phase exists to produce, and a phase
 * whose fact cannot be read answers `unknown` rather than `no`: a bounded
 * staff grant answers 404 for an org outside its scope, and reporting that as
 * "not done" would send somebody to redo work that is already there.
 *
 * Reads only, and reads that are cheap: a fresh org has one to three
 * workspaces, so the per-workspace detail fan-out is bounded by
 * `WORKSPACE_DETAIL_LIMIT` rather than left to grow with a mature account.
 */

import { parseJson, request } from "../api/request.js";
import { ensureOk, type ResolvedApp, resolveApp, staffCreds } from "../apps/resolve.js";
import type { AirhouseFleetRow } from "../commands/airhouse.js";
import { fetchBuilds, liveBuild } from "../commands/apps-client.js";
import { fetchBranch } from "../commands/oltp-branch.js";
import { call, orgPath } from "../commands/oltp-client.js";
import type { Context } from "../context/resolve.js";
import { CliError, ExitCode } from "../util/errors.js";
import type { PhaseId } from "./phases.js";
import type { BootstrapTarget } from "./target.js";

/** How many workspaces of an org are read in detail before the fan-out stops. */
export const WORKSPACE_DETAIL_LIMIT = 10;

/**
 * Whether the phase's fact is there.
 *
 * `unknown` is not a softer `no`: it means the read itself did not answer, so
 * nothing should be redone on the strength of it.
 */
export type Landed = "yes" | "no" | "unknown" | "skipped";

export interface Reading {
  id: PhaseId;
  landed: Landed;
  /** What the deployment said, in one line. */
  detail: string;
  /** The one command or step that would change `no` into `yes`. */
  next?: string;
}

/** Everything the readings share, resolved once. */
export interface Facts {
  orgId?: string;
  orgName?: string;
  ownerEmail?: string;
  /** The workspace the customer's code lives in — the one with a remote. */
  workspaceId?: string;
  workspaceName?: string;
  /** Live assume-role sessions on this target, which close `/admin`. */
  assuming: number;
  app?: ResolvedApp;
}

interface OrgMeta {
  id: string;
  name: string;
  slug: string;
  workspace_count?: number;
  owner_email?: string | null;
}

interface WorkspaceRow {
  id: string;
  name: string;
  status: string;
  org_slug?: string | null;
}

interface WorkspaceDetail extends WorkspaceRow {
  git_remote_url?: string | null;
  current_revision_id?: string | null;
}

interface SecretsView {
  environment?: string;
  missing_required?: number;
  entries?: Array<{ key: string; is_set: boolean; required?: boolean }>;
}

/**
 * A read whose 404 means "not there yet" rather than "failed".
 *
 * Only `NOT_FOUND` is swallowed. An `AUTH` or `UNAVAILABLE` has to reach the
 * caller as itself, because the answer to it is a different action entirely
 * and a verify pass that reported "not done" for "your token expired" would
 * be worse than no verify pass.
 */
async function absentOnMissing<T>(read: () => Promise<T>): Promise<T | undefined> {
  try {
    return await read();
  } catch (cause) {
    if (cause instanceof CliError && cause.code === ExitCode.NOT_FOUND) return undefined;
    throw cause;
  }
}

/** A GET that reports its status rather than throwing on a refusal. */
async function getStatus(ctx: Context, path: string): Promise<{ status: number; body: string }> {
  const response = await request({
    target: ctx.target(),
    path,
    method: "GET",
    bearer: await ctx.bearer()
  });
  return { status: response.status, body: response.body };
}

async function getJsonOrUndefined<T>(ctx: Context, path: string): Promise<T | undefined> {
  const { status, body } = await getStatus(ctx, path);
  if (status === 404) return undefined;
  if (status < 200 || status >= 300) {
    throw new CliError(`GET ${path} failed (${status})`, {
      code: status === 401 || status === 403 ? ExitCode.AUTH : ExitCode.UNAVAILABLE,
      detail: body.trim().slice(0, 300) || undefined
    });
  }
  return (parseJson(body) ?? undefined) as T | undefined;
}

/**
 * A reading of an admin surface, which a live assume-role session closes.
 *
 * `block_admin_while_acting` answers 403 on every `/api/admin/*` route while
 * the caller is acting as a tenant, so a read that would otherwise be the
 * truth is simply unavailable. That is `unknown` — not `no`, and not an exit
 * code: a staff member who assumed into the org to do `git-link` and then
 * re-ran `verify` would otherwise get `exit 4` and a raw 403 instead of a
 * report. Only an admin 403 *during a session* is converted; every other
 * failure, and a 403 with no session (a missing capability), still reaches
 * the caller as itself.
 */
async function adminReading(
  facts: Facts,
  id: PhaseId,
  read: () => Promise<Reading>
): Promise<Reading> {
  try {
    return await read();
  } catch (cause) {
    if (facts.assuming > 0 && cause instanceof CliError && cause.code === ExitCode.AUTH) {
      return {
        id,
        landed: "unknown",
        detail: `not readable: ${facts.assuming} live assume-role session(s) close /admin on this target`,
        next: "oxyc assume end, then re-run — `oxyc bootstrap plan` shows the stage boundaries"
      };
    }
    throw cause;
  }
}

/** The `unknown` reading [`adminReading`] produces, for a caller that has none to wrap. */
function closedByAssume(facts: Facts, id: PhaseId): Reading {
  return {
    id,
    landed: "unknown",
    detail: `not readable: ${facts.assuming} live assume-role session(s) close /admin on this target`,
    next: "oxyc assume end, then re-run — `oxyc bootstrap plan` shows the stage boundaries"
  };
}

/** [`adminReading`] for a list: `undefined` when a session closed the surface. */
async function adminList<T>(facts: Facts, read: () => Promise<T[]>): Promise<T[] | undefined> {
  return adminValue(facts, read);
}

/** [`adminReading`] for a plain value: `undefined` when a session closed the surface. */
async function adminValue<T>(facts: Facts, read: () => Promise<T>): Promise<T | undefined> {
  try {
    return await read();
  } catch (cause) {
    if (facts.assuming > 0 && cause instanceof CliError && cause.code === ExitCode.AUTH) {
      return undefined;
    }
    throw cause;
  }
}

/** Rows out of either envelope the admin lists use: `{items: […]}` or a bare array. */
function rowsOf<T>(doc: unknown): T[] {
  if (Array.isArray(doc)) return doc as T[];
  const items = (doc as { items?: unknown } | undefined)?.items;
  return Array.isArray(items) ? (items as T[]) : [];
}

export async function liveAssumeSessions(ctx: Context): Promise<number> {
  const doc = await getJsonOrUndefined<unknown>(ctx, "/api/assume/current");
  return rowsOf<unknown>(doc).length;
}

/**
 * The org, by slug, or `undefined`.
 *
 * `/api/admin/orgs-meta` first because it is the staff read that carries the
 * owner and the workspace count; `/api/orgs` is the fallback for a caller
 * without `PlatformOrgs`, which can still see an org it is a member of. An
 * absent org and an org outside a bounded grant are indistinguishable here on
 * purpose — the server makes them so (404, not 403).
 */
export async function findOrg(ctx: Context, slug: string): Promise<OrgMeta | undefined> {
  const { status, body } = await getStatus(
    ctx,
    `/api/admin/orgs-meta?search=${encodeURIComponent(slug)}&page_size=200`
  );
  // 403 and 404 are both "ask the member route instead": the caller holds no
  // `PlatformOrgs`, or an assume session has closed `/admin`, or the org is
  // outside a bounded grant. **Anything else is not an answer about the org**
  // and must reach the caller as itself — a swallowed 502 here reads as
  // `org: no`, and `run --yes` then posts a second `POST /api/admin/orgs`.
  // That is `absentOnMissing`'s contract, above, and it applies to a status
  // read by hand just as much.
  if (status >= 200 && status < 300) {
    const fromAdmin = rowsOf<OrgMeta>(parseJson(body) ?? undefined).find((o) => o.slug === slug);
    if (fromAdmin) return fromAdmin;
  } else if (status !== 403 && status !== 404) {
    throw new CliError(`GET /api/admin/orgs-meta failed (${status})`, {
      code: status === 401 ? ExitCode.AUTH : ExitCode.UNAVAILABLE,
      detail: body.trim().slice(0, 300) || undefined,
      remedy:
        "this says nothing about whether the org exists, so bootstrap stops rather than offering to create one"
    });
  }
  const mine = await getJsonOrUndefined<unknown>(ctx, "/api/orgs");
  return rowsOf<OrgMeta>(mine).find((o) => o.slug === slug);
}

async function orgWorkspaces(ctx: Context, orgId: string): Promise<WorkspaceDetail[]> {
  const page = await getJsonOrUndefined<unknown>(
    ctx,
    `/api/admin/workspaces-meta?org_id=${encodeURIComponent(orgId)}&page_size=200`
  );
  const rows = rowsOf<WorkspaceRow>(page).slice(0, WORKSPACE_DETAIL_LIMIT);
  const details: WorkspaceDetail[] = [];
  for (const row of rows) {
    const detail = await getJsonOrUndefined<WorkspaceDetail>(
      ctx,
      `/api/admin/workspaces/${encodeURIComponent(row.id)}/detail`
    );
    details.push(detail ?? row);
  }
  return details;
}

/** The workspace a standup is about: the one holding the repository. */
function repositoryWorkspace(workspaces: WorkspaceDetail[]): WorkspaceDetail | undefined {
  return workspaces.find((w) => Boolean(w.git_remote_url));
}

// ── one reading per phase ───────────────────────────────────────────────────

export interface ReadResult {
  facts: Facts;
  readings: Map<PhaseId, Reading>;
}

/**
 * Read every phase in `wanted`, in dependency order, sharing the facts.
 *
 * A phase whose dependency has not landed is read anyway where the read is
 * independent, and reported `unknown` where it is not — there is no useful
 * answer to "is the Airhouse tenant there" without a workspace id.
 */
export async function readAll(
  ctx: Context,
  target: BootstrapTarget,
  wanted: Set<PhaseId>
): Promise<ReadResult> {
  const readings = new Map<PhaseId, Reading>();
  const facts: Facts = { assuming: await liveAssumeSessions(ctx) };
  const put = (reading: Reading) => readings.set(reading.id, reading);

  // `repo` is the one local phase: GitHub is the authority, and this pass does
  // not shell out to `gh`. The deployment's own evidence for it is `git-link`.
  // `skipped`, not `unknown`: this pass deliberately asks only the deployment,
  // so a GitHub fact is out of scope rather than unreadable — and `unknown`
  // would make the exit code of a healthy account indistinguishable from one
  // whose reads failed.
  if (wanted.has("repo")) {
    put({
      id: "repo",
      landed: "skipped",
      detail: `not read from here — ${target.repo} is a GitHub fact; the deployment's evidence for it is \`git-link\``,
      next: `gh repo view ${target.repo} --json pushedAt   (and \`oxyc list\` for the customer topic)`
    });
  }

  const org = await findOrg(ctx, target.slug);
  if (org) {
    facts.orgId = org.id;
    facts.orgName = org.name;
    facts.ownerEmail = org.owner_email ?? undefined;
  }
  if (wanted.has("org")) {
    put(
      org
        ? {
            id: "org",
            landed: "yes",
            detail: `${org.name} (${org.id})${org.owner_email ? `, owner ${org.owner_email}` : ", no owner on the row"}`
          }
        : {
            id: "org",
            landed: "no",
            detail: `no organization with slug '${target.slug}' is visible on ${ctx.target()}`,
            next: `oxyc bootstrap run ${target.slug} --owner <email> --phase org`
          }
    );
  }

  if (wanted.has("billing"))
    put(await adminReading(facts, "billing", () => billingReading(ctx, target, facts)));

  if (wanted.has("oltp"))
    put(await adminReading(facts, "oltp", () => oltpReading(ctx, target, facts)));

  // `undefined` means the list could not be read, which is different from an
  // org with no workspaces: the first is `unknown`, the second is `no`.
  const workspaces = facts.orgId
    ? await adminList(facts, () => orgWorkspaces(ctx, facts.orgId as string))
    : [];
  const repoWorkspace = workspaces && repositoryWorkspace(workspaces);
  if (repoWorkspace) {
    facts.workspaceId = repoWorkspace.id;
    facts.workspaceName = repoWorkspace.name;
  }

  if (wanted.has("git-link"))
    put(
      workspaces
        ? gitLinkReading(target, facts, workspaces, repoWorkspace || undefined)
        : closedByAssume(facts, "git-link")
    );
  if (wanted.has("workspace"))
    put(
      workspaces
        ? workspaceReading(facts, repoWorkspace || undefined)
        : closedByAssume(facts, "workspace")
    );
  if (wanted.has("airhouse"))
    put(await adminReading(facts, "airhouse", () => airhouseReading(ctx, facts)));

  // Only a 404 means "no such app". Anything else — a 5xx, an expired token —
  // is not evidence of absence, and reporting it as `no app on this
  // deployment` would send the operator off to publish one.
  const app = await adminValue(facts, () =>
    absentOnMissing(() => resolveApp(staffCreds(ctx), `${target.slug}/${target.app}`))
  );
  if (app) facts.app = app;
  if (wanted.has("app")) put(await adminReading(facts, "app", () => appReading(ctx, target, app)));
  if (wanted.has("secrets"))
    put(await adminReading(facts, "secrets", () => secretsReading(ctx, app)));
  if (wanted.has("frontline")) put(await frontlineReading(ctx, facts));

  return { facts, readings };
}

async function billingReading(
  ctx: Context,
  target: BootstrapTarget,
  facts: Facts
): Promise<Reading> {
  if (!facts.orgId) {
    return { id: "billing", landed: "unknown", detail: "no org to read a subscription for" };
  }
  const { status, body } = await getStatus(
    ctx,
    `/api/admin/orgs/${encodeURIComponent(facts.orgId)}/billing/subscription`
  );
  if (status === 404) {
    return {
      id: "billing",
      landed: "no",
      detail:
        "no subscription — the org is billing-Incomplete, so every route behind subscription_guard answers 402 (including the GitHub import)",
      next: `the Global Owner provisions one: oxyc api /api/admin/orgs/${facts.orgId}/billing/provision-subscription -X POST   (Admin → Billing)`
    };
  }
  if (facts.assuming > 0 && (status === 401 || status === 403)) {
    // `block_admin_while_acting` answers 403 on every /api/admin/* route during
    // a session, so this refusal is the session, not the caller's role.
    return closedByAssume(facts, "billing");
  }
  if (status === 401 || status === 403) {
    // This route is behind `oxy_owner_guard_middleware`, which refuses
    // everyone but the Global Owner. The person standing an account up holds
    // `PlatformOrgCreate` and is usually a Global Admin, so this is the NORMAL
    // case — and it must not read as "probably does not apply", because this
    // is the phase that answers 402 on the GitHub import.
    return {
      id: "billing",
      landed: "unknown",
      detail: `only the Global Owner can read a subscription (${status}) — this says nothing about whether the org has one`,
      next: `ask the Global Owner, or test it directly: oxyc api /api/orgs/${facts.orgId}/workspaces --env ${ctx.flags.env ?? "production"}   — a 402 means the subscription is missing`
    };
  }
  if (status < 200 || status >= 300) {
    // A deployment with no Stripe configuration cannot answer this, and on one
    // like that `subscription_guard` short-circuits — so the phase does not
    // apply rather than having failed. Only for a 5xx: a refusal is handled
    // above, and conflating the two told an admin the opposite of the truth.
    return {
      id: "billing",
      landed: "unknown",
      detail: `the billing service did not answer (${status}) — on a deployment with no Stripe configuration, or with the \`billing\` flag off, subscription_guard short-circuits and this phase does not apply`,
      next: `oxyc api /api/orgs/${facts.orgId}/workspaces --env ${ctx.flags.env ?? "production"}   — a 402 means it does apply`
    };
  }
  const detail = (parseJson(body) ?? {}) as { status?: string; cancel_at_period_end?: boolean };
  const subscriptionStatus = detail.status ?? "unknown";
  const grants = subscriptionStatus === "active" || subscriptionStatus === "trialing";
  // `grants_access` (crates/billing/src/state.rs) also grants `past_due`
  // while it is inside `grace_period_ends_at`, and that window is not on this
  // response — so a paying org in grace must not be reported `no`, which
  // would exit 9 on an account that is working.
  const mayStillGrant = subscriptionStatus === "past_due";
  if (!grants && mayStillGrant) {
    return {
      id: "billing",
      landed: "unknown",
      detail: `subscription past_due — still granting access if it is inside its grace window, which this response does not carry`,
      next: `Admin → Billing for ${target.slug} for the grace deadline, or test it: oxyc api /api/orgs/${facts.orgId}/workspaces --env ${ctx.flags.env ?? "production"}   — a 402 means the grace window has closed`
    };
  }
  return {
    id: "billing",
    landed: grants ? "yes" : "no",
    detail: `subscription ${subscriptionStatus}${detail.cancel_at_period_end ? ", cancelling at period end" : ""}`,
    ...(grants
      ? {}
      : {
          next: `Admin → Billing for ${target.slug}: a status other than active does not grant access`
        })
  };
}

interface OltpInfo {
  is_provisioned: boolean;
  database: string;
  host: string;
  schemas?: Array<{ schema: string; writer_name?: string }>;
  analyst_ready?: boolean;
}

async function oltpReading(ctx: Context, target: BootstrapTarget, facts: Facts): Promise<Reading> {
  if (!facts.orgId) {
    return { id: "oltp", landed: "unknown", detail: "no org to read a store for" };
  }
  const provisionCommand = `oxyc oltp provision --org ${target.slug} ${target.writers
    .map((w) => `--writer ${w}`)
    .join(" ")} --branch staging`;
  const info = await absentOnMissing(() =>
    call<OltpInfo>(ctx, "GET", orgPath(facts.orgId as string))
  );
  if (!info?.is_provisioned) {
    return {
      id: "oltp",
      landed: "no",
      detail: "no OLTP database for this org",
      next: provisionCommand
    };
  }
  const schemas = new Set((info.schemas ?? []).map((s) => s.schema));
  // The schema name is what the server derives from the writer spec, so the
  // check is on the SPEC's tail rather than on a name this file re-derives.
  const missing = target.writers.filter((spec) => {
    const [kind, name] = spec.split(":");
    const expected = kind === "app" ? `app_${name}` : `raw_${name}`;
    return !schemas.has(expected.replace(/-/g, "_"));
  });
  const branch = await absentOnMissing(() => fetchBranch(ctx, facts.orgId as string, "staging"));
  const gaps = [
    ...missing.map((m) => `writer ${m} has no schema`),
    branch?.provisioned ? "" : "no staging branch"
  ].filter(Boolean);
  return {
    id: "oltp",
    landed: gaps.length === 0 ? "yes" : "no",
    detail:
      gaps.length === 0
        ? `${info.database} on ${info.host}, ${schemas.size} schema(s), staging branch cut`
        : `${info.database} on ${info.host} — ${gaps.join("; ")}`,
    ...(gaps.length === 0 ? {} : { next: provisionCommand })
  };
}

function gitLinkReading(
  target: BootstrapTarget,
  facts: Facts,
  workspaces: WorkspaceDetail[],
  repoWorkspace: WorkspaceDetail | undefined
): Reading {
  if (!facts.orgId) {
    return { id: "git-link", landed: "unknown", detail: "no org to read workspaces for" };
  }
  if (repoWorkspace) {
    return {
      id: "git-link",
      landed: "yes",
      detail: `${repoWorkspace.name} (${repoWorkspace.id}) → ${repoWorkspace.git_remote_url}`
    };
  }
  const blank = workspaces.map((w) => w.name).join(", ") || "none";
  return {
    id: "git-link",
    landed: "no",
    detail: `${workspaces.length} workspace(s) and not one with a remote: ${blank}`,
    next:
      `oxyc assume start --org ${target.slug} --reason "standing the account up", then import ${target.repo}: ` +
      `oxyc api /api/orgs/${facts.orgId}/github/namespaces   (create one if the list is empty) and ` +
      `oxyc api /api/orgs/${facts.orgId}/onboarding/github -X POST -f namespace_id=<id> -F repo_id=<numeric> -f branch=main`
  };
}

function workspaceReading(facts: Facts, repoWorkspace: WorkspaceDetail | undefined): Reading {
  if (!repoWorkspace) {
    return {
      id: "workspace",
      landed: "unknown",
      detail: "no workspace with a remote to read — `git-link` is the phase that makes one"
    };
  }
  const ready = repoWorkspace.status.toLowerCase() === "ready";
  const compiled = Boolean(repoWorkspace.current_revision_id);
  return {
    id: "workspace",
    landed: ready && compiled ? "yes" : "no",
    detail: `${repoWorkspace.name}: status ${repoWorkspace.status}, ${
      compiled ? `revision ${repoWorkspace.current_revision_id}` : "nothing compiled"
    }`,
    ...(ready && compiled
      ? {}
      : {
          next: compiled
            ? `oxyc api /api/admin/workspaces/${facts.workspaceId}/detail   — read \`error\` for why it is not Ready`
            : "nothing is compiled: push a commit to the workspace repository, or compile it from the Factory"
        })
  };
}

async function airhouseReading(ctx: Context, facts: Facts): Promise<Reading> {
  if (!facts.workspaceId) {
    return { id: "airhouse", landed: "unknown", detail: "no workspace to read a tenant for" };
  }
  const fleet = await getJsonOrUndefined<{ rows?: AirhouseFleetRow[] }>(ctx, "/api/admin/airhouse");
  const row = (fleet?.rows ?? []).find(
    (r) => r.workspace_id.toLowerCase() === facts.workspaceId?.toLowerCase()
  );
  if (!row || row.status === "none") {
    return {
      id: "airhouse",
      landed: "no",
      detail: row ? "no Airhouse tenant for this workspace" : "not on the Airhouse fleet page",
      next: `oxyc airhouse provision --workspace ${facts.workspaceId}`
    };
  }
  if (!row.service_account_ready) {
    return {
      id: "airhouse",
      landed: "no",
      detail: `tenant ${row.tenant_id} exists but its service account is not bound — the workspace cannot mint the credential every query uses`,
      next: `oxyc airhouse provision --workspace ${facts.workspaceId}`
    };
  }
  return {
    id: "airhouse",
    landed: "yes",
    detail: `tenant ${row.tenant_id} (${row.status}), service account bound`
  };
}

async function appReading(
  ctx: Context,
  target: BootstrapTarget,
  app: ResolvedApp | undefined
): Promise<Reading> {
  const label = `${target.slug}/${target.app}`;
  if (!app) {
    return {
      id: "app",
      landed: "no",
      detail: `no app ${label} on this deployment`,
      next: `oxyc publish --org ${target.slug} --app ${target.app} --project <workspace-uuid>   (from the app directory; the first publish needs --project)`
    };
  }
  const history = await absentOnMissing(() => fetchBuilds(staffCreds(ctx), app.appId));
  const live = history ? liveBuild(history) : undefined;
  if (!live) {
    return {
      id: "app",
      landed: "no",
      detail: `${label} is registered with no build being served`,
      next: `oxyc promote ${label}   — and note that production only serves a build staging has served (409 staging_never_served)`
    };
  }
  return {
    id: "app",
    landed: "yes",
    detail: `${label} serving build ${live.build_id}${live.commit_sha ? ` (${live.commit_sha.slice(0, 8)})` : ""}, published ${live.created_at}`
  };
}

async function secretsReading(ctx: Context, app: ResolvedApp | undefined): Promise<Reading> {
  if (!app) {
    return { id: "secrets", landed: "unknown", detail: "no app to read secrets for" };
  }
  const creds = staffCreds(ctx);
  const gaps: string[] = [];
  for (const environment of ["production", "staging"] as const) {
    const response = await request({
      target: creds.target,
      path: `/api/customer-apps/${app.appId}/secrets?environment=${environment}`,
      method: "GET",
      bearer: creds.bearer,
      headers: creds.headers
    });
    if (response.status === 404) continue;
    ensureOk(response, creds);
    const view = (parseJson(response.body) ?? {}) as SecretsView;
    const missing = view.missing_required ?? 0;
    if (missing > 0) gaps.push(`${environment}: ${missing} required key(s) unset`);
  }
  return {
    id: "secrets",
    landed: gaps.length === 0 ? "yes" : "no",
    detail:
      gaps.length === 0
        ? "every declared required key has a value in production and staging"
        : gaps.join("; "),
    ...(gaps.length === 0
      ? {}
      : {
          next: `set the values in the console (Organization → Apps → ${app.appSlug} → Secrets), or oxyc api /api/customer-apps/${app.appId}/secrets -X POST -f key=<KEY> -f value=<value> -f environment=production`
        })
  };
}

async function frontlineReading(ctx: Context, facts: Facts): Promise<Reading> {
  if (!facts.orgId) {
    return { id: "frontline", landed: "unknown", detail: "no org to read a crew for" };
  }
  // A tenant-surface read: staff without a live assume session get a refusal
  // here, which is a fact about the SESSION and not about the crew.
  const workers = await getJsonOrUndefined<unknown>(
    ctx,
    `/api/orgs/${encodeURIComponent(facts.orgId)}/frontline/workers`
  ).catch(() => undefined);
  const devices = await getJsonOrUndefined<unknown>(
    ctx,
    `/api/orgs/${encodeURIComponent(facts.orgId)}/frontline/devices`
  ).catch(() => undefined);
  if (workers === undefined && devices === undefined) {
    return {
      id: "frontline",
      landed: "unknown",
      detail:
        facts.assuming > 0
          ? "the frontline routes did not answer"
          : "the frontline routes are org-scoped and there is no live assume-role session",
      next: 'oxyc assume start --org <slug> --reason "standing the account up"'
    };
  }
  const workerCount = rowsOf<unknown>(workers).length;
  const deviceCount = rowsOf<unknown>(devices).length;
  const landed = workerCount > 0 && deviceCount > 0;
  return {
    id: "frontline",
    landed: landed ? "yes" : "no",
    detail: `${workerCount} worker(s), ${deviceCount} kiosk(s)`,
    ...(landed
      ? {}
      : {
          next: "enrol a worker (POST /api/orgs/{org_id}/frontline/workers) and register a kiosk (POST …/devices), then open its one-time enrol link on the tablet"
        })
  };
}
