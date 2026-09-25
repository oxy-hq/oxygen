import type { AppVisibility } from "./appAccess";

/**
 * Tagged source spec — matches the backend `SourceSpec` enum. The build store
 * (`s3`) is the only source; `v0` and `local` were removed on 2026-09-17.
 */
export type CustomAppSource = { type: "s3" };

export interface CustomApp {
  id: string;
  /** URL slug, unique within org. Auto-derived from name on create. */
  slug: string;
  name: string;
  org_id: string;
  /** Denormalised from organizations.slug for sync URL construction. */
  org_slug: string;
  project_id: string;
  branch: string;
  source_repo: string;
  status: string;
  /**
   * Canonical pretty URL `<base>/customer-apps/<org_slug>/<app_slug>/`.
   * Always set.
   */
  url: string;
  /**
   * Subdomain URL, e.g.
   * `https://mars--command-center.customer-apps-dev.oxygen-hq.com/`.
   *
   * There is no env var for this — the server auto-derives the zone from
   * `OXY_API_URL` (admin host `app{-env}` → `customer-apps{-env}`).
   * `null` when that derivation fails (unset `OXY_API_URL`, `localhost`, or
   * a custom-branded host) — admin UI shows whichever URLs are present.
   */
  url_subdomain: string | null;
  /** Manifest-derived app glyph URL (`<url><manifest.icon>`), or absent when the
   *  app declares no `icon`. Same source the homepage launcher uses (there is no
   *  favicon.ico probe) — render with a monogram fallback via <AppMark>. See the
   *  oxy-app-visual-identity skill. */
  icon_url?: string;
  /** Manifest-derived preview-image URL (`<url><manifest.art>`), or absent. */
  art_url?: string;
  /** `s3` for every app oxy serves. A row left over from a removed source
   *  (`v0`, `local`) still carries its old value, and fails at dispatch. */
  source_type: string;
  source_config: Record<string, unknown>;
  /** PR URL set by the scaffold flow when `scaffold_pr: true` was passed. */
  bootstrap_pr_url: string | null;
  last_synced_at: string | null;
  last_deploy_at: string | null;
  /**
   * MAX(custom_app_view_event.viewed_at) — last time anyone opened
   * this app in a browser. NULL until the first view is recorded.
   * Populated on list responses (batched); detail responses leave it
   * absent because the Activity tab fetches a richer view.
   */
  last_active_at?: string | null;
  /**
   * Set when an Oxy engineer publishes the app via the admin UI. Null
   * = draft (only Oxy staff with workspace oxy-access can reach it).
   * Once set, the app appears in the customer's workspace sidebar.
   */
  published_at: string | null;
  /**
   * Stable bundle identifier in the customer-apps git repo
   * (`<repo-org>/<repo-slug>`). Drives the S3 key
   * (`customer-apps/<repo_path>/{draft,published}/...`). Defaults to
   * `<org_slug>/<slug>`; null only on rows from a removed source kind.
   */
  repo_path: string | null;
  created_at: string;
  updated_at: string;
  /**
   * Email of whoever last promoted a build for this app (published /
   * made-live / rolled back). Populated on list responses via a batched
   * lookup; absent on cheap single responses. Drives the "promoted by" line.
   */
  last_promoted_by_email?: string | null;
  /** When that last promotion happened. Set from the model on every response. */
  last_promoted_at?: string | null;
  /**
   * The build this app is currently serving records no usable git source —
   * missing repo, missing commit, or both. Nobody can get from the running
   * app to its code, so it can't be fixed or handed over. Populated on list
   * responses (batched); absent elsewhere. False for apps with no build at
   * all — nothing deployed, nothing orphaned.
   */
  source_unrecorded?: boolean;
  /**
   * Who put the live build there: `ci` when trusted-publishing CI (GitHub OIDC,
   * `oxyc init-ci --promote`) published it, `person` when a user did — including
   * a CI job holding a long-lived token, which records the human who minted it.
   * Absent/null when nothing is live or the build names no publisher. Populated
   * on list responses (batched); absent elsewhere. Drives "Not from CI".
   */
  live_published_via?: "ci" | "person" | null;
}

/**
 * Per-app outcome in a batch publish / unpublish / delete response.
 * `ok = false` carries a short reason so the UI can name which apps failed.
 */
export interface BatchAppItemResult {
  id: string;
  ok: boolean;
  error?: string;
}

/**
 * Aggregate result of a batch app mutation. Returned with HTTP 200 whenever
 * the request is well-formed — per-app failures live in `results`, not the
 * status code — so the caller can report "published 4, 1 failed" from one
 * response.
 */
export interface BatchAppResult {
  succeeded: number;
  failed: number;
  results: BatchAppItemResult[];
}

/**
 * Summary returned by `GET /api/{workspaceId}/custom-apps`. Lighter
 * than [`CustomApp`] because the workspace sidebar only needs the
 * label + the URL it links to.
 */
export interface CustomAppSummary {
  id: string;
  slug: string;
  name: string;
  org_slug: string;
  url: string;
  published_at: string;
  description?: string;
  default_agent?: string;
  suggested_questions?: string[];
  art_url?: string;
  icon_url?: string;
  /** Launcher-card status line, e.g. "23 stores · sales +33.5% YoY · live". */
  status?: string;
  /**
   * Who can open the app. Every card in this list is one the viewer may open —
   * the server drops the rest — so this is not an access decision, it's what
   * lets an org officer see an app's access state and change it from the grid.
   */
  visibility: AppVisibility;
}

/**
 * One row of an app's versioned build history (new publish pipeline).
 * `id` is the build's primary key — pass it to rollback. `is_draft` /
 * `is_published` mark which channel currently points at this build.
 */
export interface AppBuild {
  id: string;
  build_id: string;
  created_at: string;
  is_draft: boolean;
  is_published: boolean;
  /** Email of the app-admin who published this build. Null for builds
   * created before publisher tracking, or via legacy paths. */
  published_by_email?: string | null;
  /** Git provenance captured by `oxyc publish`. All null for legacy /
   * non-git builds. `source_repo` is the raw remote URL — the UI normalizes
   * it to a GitHub link against `commit_sha`. */
  source_repo?: string | null;
  commit_sha?: string | null;
  source_branch?: string | null;
}

/**
 * `GET /{id}/builds` response: the build history plus who last promoted a
 * build to live (promote draft or Make Live/rollback) — distinct from each
 * build's original `published_by_email`.
 */
export interface AppBuildHistory {
  builds: AppBuild[];
  promoted_by_email?: string | null;
  promoted_at?: string | null;
}

/** One Oxy Function in an app's active build, projected from its manifest.
 *  See `GET /customer-apps/{id}/functions`. */
export interface AppFunctionSummary {
  name: string;
  /** Effective HTTP-invocable surface. */
  route: boolean;
  /** Cron expression when the function declares a schedule. */
  schedule: string | null;
  timezone: string | null;
  /** Wired as an Airway pipeline transform step. */
  airway: boolean;
  timeout_seconds: number | null;
  /** Background-run retry policy, when declared. */
  retries: {
    max_attempts: number;
    min_timeout_ms: number | null;
    max_timeout_ms: number | null;
  } | null;
  /** May write app-scoped secrets via `ctx.secrets.set`. */
  secrets_write: boolean;
  /** Databases the function may write to via `ctx.warehouse`. */
  destinations: string[];
  /** Author-declared example input (manifest `inputExample`) — prefilled into
   *  the "Run now" params box so operators know what to pass. */
  input_example?: unknown;
}

/** Where a secret's declaration came from. `undeclared` means it is stored but
 *  nothing in the active build asks for it — normal for a key a function wrote
 *  itself via `ctx.secrets.set`, such as a refreshed OAuth token. */
export type AppSecretSource = "manifest" | "webhook" | "undeclared";

/** One key in an app's secrets view: what the build asks for, unioned with what
 *  is actually stored. A value is never included — reveal is a separate call. */
export interface AppSecretEntry {
  /** Bare key (`STRIPE_API_KEY`); the `apps/<id>/` storage prefix never leaks. */
  key: string;
  is_set: boolean;
  /** The active build asks for this key. */
  declared: boolean;
  /** Declared `required` and worth flagging while unset. */
  required: boolean;
  source: AppSecretSource;
  description?: string;
  /** Present only when set — the underlying project-secret row. */
  secret_id?: string;
  updated_at?: string;
  updated_by_email?: string;
}

/** An app's secrets, reconciled against what its active build declares.
 *  See `GET /customer-apps/{id}/secrets`. */
export interface AppSecrets {
  app_id: string;
  app_slug: string;
  app_name: string;
  entries: AppSecretEntry[];
  /** Required keys with nothing stored — the "not ready" count. */
  missing_required: number;
  /** The build the declarations were read from (published, else draft). */
  declaring_build_id?: string;
  /** The manifest has an `env` block that could not be parsed. Shown rather
   *  than swallowed, so a typo doesn't look like an app declaring nothing. */
  declaration_error?: string;
}

/** One recorded invocation of a function (route / schedule / manual job). */
export interface FunctionInvocation {
  id: string;
  /** `route` | `schedule` | `airway`. */
  mode: string;
  /** `running` | `success` | `error` | `cancelled` | `timeout` | `shed`.
   *  Recorded verbatim: `success` only means the handler returned without
   *  throwing — read `failed` for whether it worked. */
  status: string;
  /** The platform counted it as a failure (the pager's rule): an `error` or
   *  `timeout`, or a `success` that answered 5xx or caught a failed ctx call. */
  failed: boolean;
  /** The HTTP status the function answered — stored only for keyed route
   *  calls, so usually null. */
  result_status: number | null;
  duration_ms: number | null;
  error: string | null;
  created_at: string;
  has_result: boolean;
}

/** A function-job run's status + persisted logs, for the trigger-and-watch loop.
 *  See `GET /customer-apps/{id}/function-runs/{run_id}`. */
export interface FunctionRunDetail {
  run_id: string;
  /** `running` | `done` | `failed` | `cancelled` | … */
  status: string | null;
  /** `scheduled` | `manual`. */
  trigger: string | null;
  answer: string | null;
  error: string | null;
  logs: { seq: number; level: string; message: string }[];
}

export interface CreateAppRequest {
  name: string;
  org_id: string;
  project_id: string;
  branch?: string;
  /** Optional explicit slug. If absent, derived from `name` and deduped. */
  slug?: string;
  /** Defaults to `{ type: "s3" }` on the server when omitted. */
  source?: CustomAppSource;
  /**
   * When true, the backend opens a PR on
   * `OXY_CUSTOMER_APPS_REPO` scaffolding `apps/<org>/<slug>/` before
   * returning. The PR URL ends up on the response's `bootstrap_pr_url`.
   */
  scaffold_pr?: boolean;
  /** Curated template for the scaffold PR. Defaults to "vite" server-side. */
  template_id?: string;
  /**
   * Stable bundle identifier — the `<repo-org>/<repo-slug>` path
   * under the customer-apps git repo where this bundle's source
   * lives. Drives the S3 key
   * (`customer-apps/<repo_path>/{draft,published}/...`) so the bundle
   * has the same storage path across every environment.
   *
   * Defaults server-side to the row's `<org_slug>/<slug>` pair when omitted — covers the
   * common case where the admin row's identity matches the repo
   * layout. Set explicitly when per-env slug drift would otherwise
   * put the same bundle at different S3 paths in dev vs prod.
   */
  repo_path?: string;
}

/**
 * One workspace from `GET /api/customer-apps/oxy-access`, flattened with its org
 * and its Oxy-staff LOCKDOWN state. Powers the admin Orgs / Projects browser.
 *
 * Inverted 2026-07-14: staff access is now the DEFAULT, so this lists EVERY
 * workspace and flags the ones an org has locked us out of. `accessible` is the
 * single field to read; `locked` is its negation plus audit metadata.
 */
export interface OxyAccessRow {
  workspace_id: string;
  workspace_name: string;
  org_id: string;
  org_name: string;
  org_slug: string;
  /** May Oxy staff touch this workspace's apps? */
  accessible: boolean;
  locked: boolean;
  locked_by_email: string | null;
  locked_at: string | null;
}

/**
 * Diagnostic snapshot from `GET /api/customer-apps/<org>/<app>/debug`.
 * Loose by design — the admin UI inspects it for humans; field
 * additions on the server should not break clients.
 */
export interface CustomAppDebug {
  org_slug: string;
  app_slug: string;
  app: {
    id: string;
    slug: string;
    name: string;
    status: string;
    source_type: string;
  };
  /** The channel this request resolved to — `draft` for staff previewing or
   *  an app never published, `published` otherwise. */
  channel: "draft" | "published";
  /** The build that channel points at (its row id, not the `build_id` Build
   *  history shows), or null when there is nothing to serve. */
  build: string | null;
  /** Where the manifest came from: the per-app override, or the build. */
  manifest_source: "db_override" | "bundle_file";
  /** Loose by design — server schema can grow without breaking clients. */
  manifest?: unknown;
  manifest_error: string | null;
}

// ── Custom-app storage (asset-lifecycle design, 2026-08-05) ──────────────────

/** Per-top-level-prefix split, captured during the sweeper's walk. */
export interface StoragePrefixUsage {
  bytes: number;
  objects: number;
  /** TTL class the app's policy assigns this prefix; absent = kept forever. */
  expireAfter?: string;
}

/** One app's row in the fleet storage view. */
export interface AppStorageUsageRow {
  appId: string;
  appName: string;
  appSlug: string;
  orgId: string;
  orgName: string | null;
  bytes: number;
  objectCount: number;
  /** Bytes no retention rule covers — growth nothing will reclaim. */
  untaggedBytes: number;
  /** `null` when there is no sample old enough to difference against. */
  growthBytes7d: number | null;
  prefixBreakdown: Record<string, StoragePrefixUsage> | null;
  measuredAt: string;
  /** `ok` | `partial` | `failed`. Anything but `ok` means the row is a floor. */
  measureStatus: string;
  measureDetail: string | null;
}

export interface FleetStorageResponse {
  rows: AppStorageUsageRow[];
  totalBytes: number;
  totalObjects: number;
  totalUntaggedBytes: number;
  /** Apps with no usage row yet — "never measured", not "0 bytes". */
  unmeasuredApps: number;
  totalsAreFloor: boolean;
  softLimitBytes: number | null;
  hardLimitBytes: number | null;
}

export interface StorageObject {
  key: string;
  /** Key with the silo prefix stripped. */
  path: string;
  size: number;
  contentType: string | null;
  lastModified: string | null;
  /** TTL class today's policy assigns; absent = kept forever. */
  expireAfter?: string;
}

export interface StorageRetentionRule {
  prefix: string;
  expireAfter: string | null;
}

export interface StorageBrowseResponse {
  objects: StorageObject[];
  cursor: string | null;
  hasMore: boolean;
  retentionRules: StorageRetentionRule[];
}

/**
 * A sweep is accepted, not completed — it runs in the background because
 * walking every silo can take minutes. Results land in the fleet rollup.
 */
export interface StorageSweepStarted {
  started: boolean;
}

/** One day's storage total, as held at that day's end. */
export interface StorageHistoryPoint {
  /** `YYYY-MM-DD`, UTC. */
  date: string;
  bytes: number;
  objectCount: number;
}

export interface StorageHistoryResponse {
  days: number;
  points: StorageHistoryPoint[];
}

// ── Availability (derived SLI) ───────────────────────────────────────────────

/** One measurement window from the availability endpoint. */
export interface AvailabilityWindow {
  window_minutes: number;
  total: number;
  failed: number;
  /**
   * `null` when the window carried no traffic — **not** `0`. "Nothing failed"
   * and "nothing happened" are different answers, and a chart that conflates
   * them draws a healthy flat line over a dead app.
   */
  failure_ratio: number | null;
}

/**
 * `no_opinion` is a first-class answer, not an error: an app with no traffic
 * has not been shown to work. Render it as "no data", never as a green tick.
 */
export type AvailabilityVerdict = "no_opinion" | "healthy" | "burning";

export interface AppAvailability {
  app_id: string;
  org_slug: string;
  app_slug: string;
  verdict: AvailabilityVerdict;
  /** Present only when `verdict` is `burning`. */
  severity?: "page" | "ticket";
  burn_rate?: number;
  long_window_minutes?: number;
  short_window_minutes?: number;
  /** The availability objective the burn rate is measured against, e.g. 0.99. */
  objective: number;
  windows: AvailabilityWindow[];
}

// ── Fleet health (every published app, one verdict each) ─────────────────────

/**
 * What the fleet table says about one app.
 *
 * `not_measured` and `quiet` are the two that matter. Four separate layers in
 * the backend used to turn an absent measurement into a green tick — capture
 * off, workspace never evaluated, the query erroring, traffic below the burn
 * evaluator's floor — and these two states are what stop that. Never render
 * either as healthy.
 */
export type AppHealth = "down" | "degraded" | "not_measured" | "quiet" | "operational";

export interface AppHealthRow {
  app_id: string;
  app_slug: string;
  app_name: string;
  org_id: string;
  org_slug: string;
  health: AppHealth;
  /** Why, in a line. Absent only when the app is plainly operational. */
  reason: string | null;
  /** Requests over `window_minutes`, and how many failed. Both 0 on an
   *  unmeasured app — read `health` first, not these. */
  requests: number;
  failed: number;
  /** What the same window carried a week ago, when there was a baseline.
   *  `null` means no established rhythm to compare against. */
  baseline: number | null;
  window_minutes: number;
}

export interface FleetSummary {
  down: number;
  degraded: number;
  not_measured: number;
  quiet: number;
  operational: number;
}

export interface FleetHealthResponse {
  apps: AppHealthRow[];
  summary: FleetSummary;
  /** Published apps in scope before paging. `apps.length` can be smaller for two
   *  independent reasons — the page cap and the needs-attention filter — so this
   *  is what tells a reader whether they are seeing the whole fleet. */
  total: number;
  has_more: boolean;
  evaluated_at: string;
  /** `false` when OXY_OBSERVABILITY_BACKEND is unset. Every row is then
   *  `not_measured`, and the page must say so rather than look empty. */
  observability_configured: boolean;
}

// ── Logs & client errors (per-app debuggability) ─────────────────────────────

export interface FunctionLogLine {
  timestamp: string;
  build_id: string;
  invocation_id: string;
  request_id: string;
  function_name: string;
  /** `route` | `schedule` | `airway`. */
  mode: string;
  /** `info` | `warn` | `error`. */
  level: string;
  seq: number;
  message: string;
}

export interface ClientError {
  stack_hash: string;
  error_name: string;
  message: string;
  /** Source-mapped where the build's maps allowed it, raw where they did not. */
  stack: string;
  /**
   * Whether anything in `stack` actually changed. Without it a reader cannot
   * tell real file names from a still-minified stack whose map was missing —
   * exactly the moment they would be misled.
   */
  stack_resolved: boolean;
  build_id: string;
  path: string;
  /** `error` | `unhandledrejection`. */
  kind: string;
  occurrences: number;
  sessions: number;
  first_seen: string;
  last_seen: string;
}
