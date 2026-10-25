import { PRODUCTION } from "@/types/appEnvironments";
import type { MovePlan, MoveRequest, MoveTarget } from "@/types/appPromote";
import type {
  AppBuildHistory,
  AppFunctionSummary,
  AppNotificationKinds,
  AppSecrets,
  BatchAppResult,
  ClientError,
  CreateAppRequest,
  CustomApp,
  CustomAppDebug,
  CustomAppSummary,
  CustomDomain,
  CustomDomainsResponse,
  FleetHealthResponse,
  FunctionInvocation,
  FunctionLogLine,
  FunctionRunDetail,
  InvocationCalls,
  OxyAccessRow
} from "@/types/apps";
import { apiClient } from "./axios";

/** What setting an app secret did: stored a key the app did not have, or
 *  replaced the value of one it did. */
export type AppSecretWrite = "created" | "updated";

/** The set endpoint is an upsert and says which in its status: 201 for a new
 *  key, 204 for a replaced value. */
const appSecretWrite = (status: number): AppSecretWrite => (status === 201 ? "created" : "updated");

/**
 * Customer-apps registry — gated by OXY_GLOBAL_ADMINS on the server. CRUD is
 * uuid-keyed (internal callers always know the uuid); sync uses the pretty
 * `<org_slug>/<app_slug>` path because CI calls it without ever seeing the
 * uuid.
 */
/**
 * The one place an app id becomes a URL segment. Every id-addressed call below goes
 * through it so a value that arrived as text — the dossier window's `?app=`, a pasted
 * id — is encoded before it can reshape the path; `../` encodes to `..%2F..`, which
 * names nothing.
 */
const appPath = (id: string) => `/customer-apps/${encodeURIComponent(id)}`;

export const CustomAppsService = {
  /**
   * Fleet health — every published app the caller can see, worst first.
   *
   * Server-side scope filters the rows, so a bounded grant sees only its own
   * orgs. `needsAttention` drops the quiet and operational rows; unmeasured
   * apps survive it, because "we are not watching this" is exactly what an
   * operator needs to see.
   */
  async fleetHealth(
    options: { limit?: number; offset?: number; needsAttention?: boolean } = {}
  ): Promise<FleetHealthResponse> {
    const params = new URLSearchParams();
    if (options.limit != null) params.set("limit", String(options.limit));
    if (options.offset != null) params.set("offset", String(options.offset));
    if (options.needsAttention) params.set("needs_attention", "true");
    const suffix = params.toString();
    const response = await apiClient.get(
      `/customer-apps/fleet-health${suffix ? `?${suffix}` : ""}`
    );
    return response.data;
  },

  /**
   * Paged admin list. The server orders rows by `updated_at` DESC so
   * page 0 is "what got touched most recently"; `next_offset` is null
   * when we've walked off the end. Callers without an explicit page
   * use the server default size (50) — fine for the first paint, and
   * the infinite-query hook walks back from there.
   */
  async list(
    options: { limit?: number; offset?: number; id?: string } = {}
  ): Promise<{ items: CustomApp[]; next_offset: number | null }> {
    const params = new URLSearchParams();
    if (options.limit != null) params.set("limit", String(options.limit));
    if (options.offset != null) params.set("offset", String(options.offset));
    if (options.id) params.set("id", options.id);
    const suffix = params.toString();
    const response = await apiClient.get(`/customer-apps${suffix ? `?${suffix}` : ""}`);
    return response.data;
  },

  /**
   * One app's admin detail (`GET /api/customer-apps/{id}`). The only response
   * that carries `staging_url` — the list leaves it absent rather than pay an
   * `app_environments` read per row — so the console's Draft view reads it
   * from here.
   */
  async get(id: string): Promise<CustomApp> {
    const response = await apiClient.get(`${appPath(id)}`);
    return response.data;
  },

  /**
   * Workspace-scoped published list — the lighter `CustomAppSummary` shape
   * the HQ launcher + workspace rail render, hitting the workspace router's
   * `/{workspaceId}/custom-apps`. Distinct from the paged admin `list` above.
   */
  async listForWorkspace(workspaceId: string): Promise<CustomAppSummary[]> {
    const response = await apiClient.get(`/${workspaceId}/custom-apps`);
    return response.data;
  },

  /**
   * The same list with each card's `verdict` (`?verdict=true`): an observability read
   * on the server, so only the surface that shows health asks — the workspace rail,
   * which fetches the plain list on every page, must not.
   */
  async listForWorkspaceWithVerdict(workspaceId: string): Promise<CustomAppSummary[]> {
    const response = await apiClient.get(`/${workspaceId}/custom-apps`, {
      params: { verdict: true }
    });
    return response.data;
  },

  /**
   * Every workspace that granted Oxy access, platform-wide, for the admin
   * org/project browser. App-admin gated (same surface as the registry).
   */
  async listOxyAccess(): Promise<OxyAccessRow[]> {
    const response = await apiClient.get("/customer-apps/oxy-access");
    return response.data;
  },

  async create(req: CreateAppRequest): Promise<CustomApp> {
    const response = await apiClient.post("/customer-apps", req);
    return response.data;
  },

  async delete(id: string): Promise<void> {
    await apiClient.delete(`${appPath(id)}`);
  },

  /**
   * Publish the app. Until published, only Oxy staff (with workspace
   * oxy-access) can reach the URL; once published, it surfaces in
   * the owning workspace's sidebar.
   *
   * Every admin app route is the one `/customer-apps/*` mount (owner-or-app-admin,
   * `PlatformApps` capability, app scope, refused while acting as a tenant) —
   * publishing is a normal Oxy-engineer automation.
   */
  async publish(id: string): Promise<CustomApp> {
    const response = await apiClient.post(`${appPath(id)}/publish`);
    return response.data;
  },

  async unpublish(id: string): Promise<CustomApp> {
    const response = await apiClient.delete(`${appPath(id)}/publish`);
    return response.data;
  },

  /**
   * Promote the newest build on each of `ids` — the fleet's batch action.
   * Best-effort per id server-side: one app failing never aborts the rest, and
   * the response carries a per-app outcome so the UI can report "promoted 4,
   * 1 failed" from a single call. The sibling batch publish / unpublish /
   * delete endpoints still exist on the server; nothing in the web-app calls
   * them since the selectable apps table went, so they have no client here.
   */
  async batchPromoteLatest(ids: string[]): Promise<BatchAppResult> {
    const response = await apiClient.post("/customer-apps/batch/promote-latest", { ids });
    return response.data;
  },

  /**
   * Versioned build history for the new publish pipeline, newest first.
   * Empty for legacy s3/local/v0 rows never published via `oxyc publish`.
   */
  async listBuilds(id: string): Promise<AppBuildHistory> {
    const response = await apiClient.get(`${appPath(id)}/builds`);
    return response.data;
  },

  /**
   * Make a retained build live. Server-side it goes through the one
   * production move: refused (409 `staging_never_served`) for a build staging
   * never served, and the build's pending migrations applied before the
   * pointer moves.
   */
  async rollback(id: string, buildId: string): Promise<CustomApp> {
    const response = await apiClient.post(`${appPath(id)}/rollback`, {
      build_id: buildId
    });
    return response.data;
  },

  // ── Oxy Functions (manage / debug) ─────────────────────────────────────────

  /** The app's Oxy Functions in its active build, with their manifest config. */
  async listFunctions(id: string): Promise<AppFunctionSummary[]> {
    const response = await apiClient.get(`${appPath(id)}/functions`);
    return response.data;
  },

  /** Recent invocations of one function (newest first), for the debug history. */
  async listFunctionInvocations(id: string, name: string): Promise<FunctionInvocation[]> {
    const response = await apiClient.get(
      `${appPath(id)}/functions/${encodeURIComponent(name)}/invocations`
    );
    return response.data;
  },

  /** A single function-job run's status + persisted logs (trigger-and-watch). */
  async getFunctionRun(id: string, runId: string): Promise<FunctionRunDetail> {
    const response = await apiClient.get(
      `${appPath(id)}/function-runs/${encodeURIComponent(runId)}`
    );
    return response.data;
  },

  /** One invocation's `ctx.*` call trace — the waterfall. 404
   *  `invocation_not_found` for an id that is not this app's. */
  async getInvocationCalls(id: string, invocationId: string): Promise<InvocationCalls> {
    const response = await apiClient.get(
      `${appPath(id)}/invocations/${encodeURIComponent(invocationId)}/calls`
    );
    return response.data;
  },

  /** Trigger a one-off background run of a function as a job, optionally with
   *  JSON input params handed to the function as its request body. Returns the
   *  run id to watch via `getFunctionRun`. */
  async runFunction(id: string, name: string, input?: unknown): Promise<{ run_id: string }> {
    const response = await apiClient.post(
      `${appPath(id)}/functions/${encodeURIComponent(name)}/runs`,
      // `undefined` → axios sends no body → the function runs with no params.
      input ?? undefined
    );
    return response.data;
  },

  // ── Test notifications (sent as the app, to the caller alone) ──────────────

  /** The kinds the app's live build declares, and whether it can notify you. */
  async listNotificationKinds(id: string): Promise<AppNotificationKinds> {
    const response = await apiClient.get(`${appPath(id)}/notification-kinds`);
    return response.data;
  },

  /** Send yourself one notification of `kind` as the app: push and the inbox,
   *  with your mutes applied. One per app every 10 seconds, else a 429. */
  async sendTestNotification(id: string, kind: string): Promise<{ id: string; kind: string }> {
    const response = await apiClient.post(`${appPath(id)}/notification-test`, { kind });
    return response.data;
  },

  // ── App-scoped secrets (`apps/<app_id>/<KEY>` — what `ctx.env` reads) ──────

  /** The app's secrets in `environment` (production by default): what the
   *  build that environment serves declares, unioned with what is stored
   *  there. Values are never included. Anything but production is staff only,
   *  and answers 403 to a caller who may not open non-production. */
  async listSecrets(
    id: string,
    environment: string = PRODUCTION,
    build?: string
  ): Promise<AppSecrets> {
    // `build` (an `app_builds.id`) reconciles against that build's declarations
    // instead of the one the environment serves: "what would be missing if this
    // build went live here", asked before a promote.
    const response = await apiClient.get(`${appPath(id)}/secrets`, {
      params: build ? { environment, build } : { environment }
    });
    return response.data;
  },

  /** Create or rotate one key in `environment`. The only write path there is —
   *  the project secrets API rejects the `/` in an app-scoped name. */
  async setSecret(
    id: string,
    key: string,
    value: string,
    environment: string = PRODUCTION
  ): Promise<AppSecretWrite> {
    const response = await apiClient.post(`${appPath(id)}/secrets`, {
      key,
      value,
      environment
    });
    return appSecretWrite(response.status);
  },

  /** Copy `from`'s stored value of `key` into `environment`, server-side: the
   *  value never reaches the browser. Staff only, and only downhill —
   *  production into staging or a sandbox, staging into a sandbox. */
  async copySecret(
    id: string,
    key: string,
    environment: string,
    from: string
  ): Promise<AppSecretWrite> {
    const response = await apiClient.post(`${appPath(id)}/secrets`, {
      key,
      environment,
      copy_from: from
    });
    return appSecretWrite(response.status);
  },

  async deleteSecret(id: string, key: string, environment: string = PRODUCTION): Promise<void> {
    await apiClient.delete(`${appPath(id)}/secrets/${encodeURIComponent(key)}`, {
      params: { environment }
    });
  },

  /** Tenant-side create/rotate, on the workspace's own route. Same handler as
   *  `setSecret`, reached with a `WorkspaceAdmin` guard instead of staff
   *  standing — this is the only app-secret verb the workspace surface needs,
   *  since the project-secrets routes already list, reveal, rotate and delete
   *  these rows by id. */
  async setWorkspaceAppSecret(
    workspaceId: string,
    appId: string,
    key: string,
    value: string
  ): Promise<AppSecretWrite> {
    const response = await apiClient.post(`/${workspaceId}/custom-apps/${appId}/secrets`, {
      key,
      value
    });
    return appSecretWrite(response.status);
  },

  /** Read one value back. Deliberately not a query — a decrypted secret should
   *  never land in the react-query cache, so this is called on demand and the
   *  result held only as long as the row is expanded. */
  async revealSecret(id: string, key: string, environment: string = PRODUCTION): Promise<string> {
    const response = await apiClient.get<{ key: string; value: string }>(
      `${appPath(id)}/secrets/${encodeURIComponent(key)}/value`,
      { params: { environment } }
    );
    return response.data.value;
  },

  /**
   * Diagnostic snapshot — what the server currently sees about the app's
   * bundle dir + resolved manifest + producer wiring. Powers the
   * Info tab in the admin detail pane.
   */
  async debug(app: Pick<CustomApp, "slug" | "org_slug">): Promise<CustomAppDebug> {
    const response = await apiClient.get(
      `/customer-apps/${encodeURIComponent(app.org_slug)}/${encodeURIComponent(app.slug)}/debug`
    );
    return response.data;
  },

  /**
   * Persisted Oxy Function output, newest first. Slug-addressed, like its
   * `/health` and `/debug` siblings.
   *
   * `requestId` narrows to one `x-oxy-request-id`. It must be a UUID: the route
   * answers anything else with a 400 rather than an empty list.
   */
  async logs(
    orgSlug: string,
    appSlug: string,
    { hours, limit, requestId }: { hours: number; limit: number; requestId?: string }
  ): Promise<FunctionLogLine[]> {
    const params = new URLSearchParams({ hours: String(hours), limit: String(limit) });
    if (requestId) params.set("request_id", requestId);
    const r = await apiClient.get(`/customer-apps/${orgSlug}/${appSlug}/logs?${params}`);
    return (r.data?.logs ?? []) as FunctionLogLine[];
  },

  /** Browser errors, grouped by stack, with source maps applied server-side. */
  async clientErrors(orgSlug: string, appSlug: string, hours = 24): Promise<ClientError[]> {
    const r = await apiClient.get(
      `/customer-apps/${orgSlug}/${appSlug}/errors?hours=${hours}&limit=100`
    );
    return (r.data?.errors ?? []) as ClientError[];
  },

  // ── Activity (usage tracking) ──────────────────────────────────────────

  async activitySummary(id: string): Promise<ActivitySummary> {
    const r = await apiClient.get(`${appPath(id)}/activity/summary`);
    return r.data as ActivitySummary;
  },

  async activityVisitors(id: string, days = 7, limit = 50): Promise<VisitorRow[]> {
    const r = await apiClient.get(`${appPath(id)}/activity/visitors?days=${days}&limit=${limit}`);
    return (r.data?.rows ?? []) as VisitorRow[];
  },

  async activityEventGroups(id: string, days = 7): Promise<EventGroupRow[]> {
    const r = await apiClient.get(`${appPath(id)}/activity/events?days=${days}`);
    return (r.data?.groups ?? []) as EventGroupRow[];
  },

  async activityEventOccurrences(
    id: string,
    eventName: string,
    days = 7,
    limit = 50
  ): Promise<EventOccurrenceRow[]> {
    const r = await apiClient.get(
      `${appPath(id)}/activity/events?days=${days}&limit=${limit}&event_name=${encodeURIComponent(eventName)}`
    );
    return (r.data?.rows ?? []) as EventOccurrenceRow[];
  },

  // ── Staging (held writes) ───────────────────────────────────────────────

  /**
   * The caller's own `app.staging.held` rows for this app, newest first
   * (`GET /customer-apps/{id}/staging/held`). 404s for a caller who may not
   * open the app's staging — same answer as an app that does not exist, so
   * the console can't use this to probe standing it doesn't have.
   */
  async listStagingHeld(id: string, limit?: number): Promise<StagingHeldEntry[]> {
    const suffix = limit != null ? `?limit=${limit}` : "";
    const response = await apiClient.get(`${appPath(id)}/staging/held${suffix}`);
    return response.data;
  },

  // ── Environments (production, staging, dev sandboxes) ───────────────────

  /** Every environment of the app (`GET /customer-apps/{id}/environments`). */
  async listEnvironments(id: string): Promise<AppEnvironment[]> {
    const response = await apiClient.get<{ environments: AppEnvironment[] }>(
      `${appPath(id)}/environments`
    );
    return response.data.environments;
  },

  /**
   * Plan (`dry_run`) or make one production move — the routes `oxyc promote` and
   * `oxyc rollback` call. A promote moves `buildId` from staging; a rollback moves
   * production back to `buildId`. The act answers 409 with the plan's `blockers`, or
   * `target_moved` when production moved since `expect_current` was read.
   */
  async moveProduction(id: string, target: MoveTarget, body: MoveRequest): Promise<MovePlan> {
    const source =
      target.act === "promote"
        ? { from: "staging", build: target.buildId }
        : { to: target.buildId };
    const response = await apiClient.post<MovePlan>(
      `${appPath(id)}/environments/production/${target.act}`,
      { ...source, ...body }
    );
    return response.data;
  },

  /** Create a dev sandbox. 409 when the name is taken or reserved. */
  async createEnvironment(id: string, name: string): Promise<AppEnvironment> {
    const response = await apiClient.post(`${appPath(id)}/environments`, { name });
    return response.data;
  },

  /**
   * Tear a sandbox down. 202: the teardown is queued, the row reads `deleting`
   * until it lands. Production and staging answer 400 (`NotASandbox`).
   */
  async deleteEnvironment(id: string, name: string): Promise<EnvironmentDeleteAccepted> {
    const response = await apiClient.delete(
      `${appPath(id)}/environments/${encodeURIComponent(name)}`
    );
    return response.data;
  },

  // ── Trusted publishers (GitHub OIDC) ─────────────────────────────────────

  /** The workflows allowed to OIDC-publish this app (`GET /customer-apps/{id}/publishers`). */
  async listPublishers(id: string): Promise<AppPublisher[]> {
    const response = await apiClient.get(`${appPath(id)}/publishers`);
    return response.data;
  },

  /** Register a workflow. 409 when that exact (owner, repo, workflow, environment) exists. */
  async registerPublisher(id: string, body: RegisterPublisherRequest): Promise<AppPublisher> {
    const response = await apiClient.post(`${appPath(id)}/publishers`, body);
    return response.data;
  },

  async deletePublisher(id: string, publisherId: string): Promise<void> {
    await apiClient.delete(`${appPath(id)}/publishers/${encodeURIComponent(publisherId)}`);
  },

  // ── Custom domains ───────────────────────────────────────────────────────

  /** The customer-owned domains this app is served on, and the CNAME target they point at. */
  async listDomains(id: string): Promise<CustomDomainsResponse> {
    const response = await apiClient.get(`${appPath(id)}/domains`);
    return response.data;
  },

  /** Claim a domain. 400 `invalid_domain`, 409 `domain_exists`. */
  async addDomain(id: string, domain: string): Promise<CustomDomain> {
    const response = await apiClient.post(`${appPath(id)}/domains`, { domain });
    return response.data;
  },

  /**
   * Look for the TXT record. 200 either way: a domain still `pending` carries
   * `last_check_error` saying why. 409 `domain_taken` when another app verified it first.
   */
  async verifyDomain(id: string, domainId: string): Promise<CustomDomain> {
    const response = await apiClient.post(
      `${appPath(id)}/domains/${encodeURIComponent(domainId)}/verify`
    );
    return response.data;
  },

  async removeDomain(id: string, domainId: string): Promise<void> {
    await apiClient.delete(`${appPath(id)}/domains/${encodeURIComponent(domainId)}`);
  },

  // ── Templates ────────────────────────────────────────────────────────────

  /** The curated scaffold templates (`GET /customer-apps/templates`). */
  async listTemplates(): Promise<AppTemplate[]> {
    const response = await apiClient.get("/customer-apps/templates");
    return response.data;
  }
};

/**
 * One environment of an app, as `custom_apps_sandboxes::EnvironmentDto` serializes
 * it. Every field is always present; one that does not apply is `null`.
 */
export interface AppEnvironment {
  name: string;
  /** `production` | `staging` | `dev`. */
  kind: string;
  /** `active` | `deleting`. */
  status: string;
  /** The publish's build id; `null` when the environment serves nothing. */
  build_id: string | null;
  build_uuid: string | null;
  semantic_revision_id: string | null;
  /** `null` for production and staging. */
  owner: { user_id: string; email: string | null } | null;
  created_at: string;
  updated_at: string;
  last_activity_at: string | null;
  /** When an idle sandbox is deleted; `null` otherwise. */
  expires_at: string | null;
  /** The environment's host; `null` without a custom-apps zone. */
  url: string | null;
  oltp_schema: unknown | null;
}

/** `DELETE …/environments/{name}` answers 202 with the queued teardown. */
export interface EnvironmentDeleteAccepted {
  name: string;
  status: string;
  teardown_run_id: string;
}

/** A GitHub workflow trusted to OIDC-publish an app (`PublisherDto`). */
export interface AppPublisher {
  id: string;
  app_id: string;
  repo_owner: string;
  repo_owner_id: number;
  repo_name: string;
  workflow_ref: string;
  environment: string;
  created_at: string;
}

export interface RegisterPublisherRequest {
  repo_owner: string;
  /** GitHub's numeric owner id — the stable identity a renamed org keeps. */
  repo_owner_id: number;
  repo_name: string;
  /** The bare workflow path, e.g. `.github/workflows/oxy-publish.yml` — what
   *  `oxyc init-ci` writes. No `@ref`: the verifier strips the ref from the token's
   *  `job_workflow_ref` before comparing, so a stored `@…` can never match. */
  workflow_ref: string;
  /** The GitHub environment the job must run in. Required: the verifier rejects a
   *  token with no `environment` claim, so an empty value matches nothing. */
  environment: string;
}

/** One curated scaffold template (`TemplateListItem`). */
export interface AppTemplate {
  id: string;
  name: string;
  description: string;
}

export interface ActivitySummary {
  total_views_7d: number;
  unique_users_7d: number;
  total_events_7d: number;
  last_viewed_at: string | null;
}

export interface VisitorRow {
  user_id: string;
  user_email: string;
  sessions: number;
  views: number;
  first_seen_at: string;
  last_seen_at: string;
  /** Role in this app on their latest view that recorded one — a snapshot, not
   *  their role today. `null` for views predating role capture. */
  app_role: "admin" | "member" | null;
  /** Role in the owning org, same basis. */
  org_role: "owner" | "admin" | "member" | null;
}

export interface EventGroupRow {
  event_name: string;
  count: number;
  last_fired_at: string;
}

export interface EventOccurrenceRow {
  id: string;
  event_name: string;
  user_email: string;
  payload: Record<string, unknown>;
  occurred_at: string;
}

/** One `app.staging.held` row, as `custom_apps_staging_held::HeldEntry` serializes it. */
export interface StagingHeldEntry {
  at: string;
  function: string;
  writes: StagingHeldWrite[];
}

/** One held call within an entry. Unknown/missing fields read as empty on the
 *  server, so an older row still lists — `op`/`note` stay optional here to
 *  match. */
export interface StagingHeldWrite {
  plane: string;
  namespace: string;
  verb: string;
  table: string;
  op: string | null;
  note: string | null;
}
