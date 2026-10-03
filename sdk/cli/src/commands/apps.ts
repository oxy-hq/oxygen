/**
 * `oxyc apps list | show | builds | health | usage` — what custom apps are
 * registered on a deployment, what each is serving, and how it is doing.
 * (`oxyc apps drift` is in `apps-drift.ts`.)
 *
 * Read-only: every request here is a GET.
 *
 *   GET /api/customer-apps?limit=&offset=                 the registry, paged by next_offset
 *   GET /api/customer-apps/{id}/builds                    build history and who promoted
 *   GET /api/customer-apps/fleet-health?limit=&offset=    every published app's verdict
 *   GET /api/customer-apps/{org}/{app}/health             deployment integrity (200 pass, 503 fail)
 *   GET /api/customer-apps/{org}/{app}/availability       request success over six windows
 *   GET /api/customer-apps/{org}/{app}/errors             browser errors, grouped by stack
 *   GET /api/customer-apps/{id}/activity/{summary,visitors,events}
 *
 * All of them sit behind the app-admin role, and a bounded staff grant sees
 * only its own organizations' rows — an app outside it is absent, not denied.
 *
 * EXIT CODES. These commands report; a failing health verdict or a quiet app is
 * an answer and exits 0. A request that failed is not an answer: it exits with
 * the code its status maps to, after printing whatever else was read, and the
 * section it belonged to says "not read" rather than showing an empty result.
 */

import { errorForResponse, parseJson, request } from "../api/request.js";
import type { Context } from "../context/resolve.js";
import * as log from "../ui/log.js";
import { type Column, heading, table } from "../ui/render.js";
import { out } from "../ui/tty.js";
import { CliError, ExitCode, usageError } from "../util/errors.js";
import { parseRemoteSlug } from "../util/git.js";
import {
  type AppRow,
  appLabel,
  type BuildSummary,
  type Conn,
  CUSTOMER_APPS,
  connect,
  draftAheadOfLive,
  fetchBuilds,
  getJson,
  listAllApps,
  liveBuild,
  mapBounded,
  openApp
} from "./apps-client.js";

/** Requests in flight at once for the one-per-app `builds` fan-out. */
export const BUILDS_CONCURRENCY = 6;

/** What a cell shows when the server has no value for it. */
const NONE = "—";

// ── formatting ─────────────────────────────────────────────────────────────

/** An RFC 3339 timestamp as `YYYY-MM-DD HH:MM`, in UTC. */
export function when(iso: string | null | undefined): string {
  if (!iso) return NONE;
  const ms = Date.parse(iso);
  if (Number.isNaN(ms)) return iso;
  return new Date(ms).toISOString().slice(0, 16).replace("T", " ");
}

export function shortSha(sha: string): string {
  return sha.slice(0, 10);
}

/** Who published a build: the person, else the CI workflow identity. */
function publisher(build: BuildSummary): string {
  return build.published_by_email ?? build.published_via ?? NONE;
}

/** `<owner>/<name>@<sha> (<branch>)` for a build, or that the build records no source. */
export function provenance(
  build: Pick<BuildSummary, "source_repo" | "commit_sha" | "source_branch">
): string {
  if (!build.source_repo || !build.commit_sha) return "source not recorded";
  const repo = parseRemoteSlug(build.source_repo) ?? build.source_repo;
  const branch = build.source_branch ? ` (${build.source_branch})` : "";
  return `${repo}@${shortSha(build.commit_sha)}${branch}`;
}

/** `360` → `6h`, `5` → `5m`. */
function windowLabel(minutes: number): string {
  return minutes % 60 === 0 ? `${minutes / 60}h` : `${minutes}m`;
}

export function printJson(value: unknown): void {
  process.stdout.write(`${JSON.stringify(value, null, 2)}\n`);
}

/** `key  value` lines with the keys padded to one width; an empty key continues the line above. */
export function printFields(fields: Array<[string, string]>): void {
  const width = Math.max(...fields.map(([key]) => key.length));
  process.stdout.write(
    `${fields.map(([key, value]) => `${key.padEnd(width)}  ${value}`.trimEnd()).join("\n")}\n`
  );
}

// ── sections that may fail independently ───────────────────────────────────

/**
 * The outcome of one request in a report that makes several.
 *
 * `show` and `health <app>` read Postgres-backed and ClickHouse-backed routes
 * side by side. One of them failing must not discard the others, and must not
 * print as an empty result either — so each outcome is carried to the renderer,
 * and `failIfAnyFailed` turns the failures into the exit code at the end.
 */
type Section<T> = { ok: true; value: T } | { ok: false; error: CliError };

async function section<T>(work: Promise<T>): Promise<Section<T>> {
  try {
    return { ok: true, value: await work };
  } catch (cause) {
    if (cause instanceof CliError) return { ok: false, error: cause };
    throw cause;
  }
}

function notRead(error: CliError): string {
  return `NOT READ — ${error.message}`;
}

interface Failure {
  section: string;
  error: string;
}

function failures(sections: Record<string, Section<unknown>>): Failure[] {
  return Object.entries(sections).flatMap(([name, outcome]) =>
    outcome.ok ? [] : [{ section: name, error: outcome.error.message }]
  );
}

/** Exit non-zero when a section's request failed, with the first failure's code. */
function failIfAnyFailed(sections: Record<string, Section<unknown>>): void {
  const failed = Object.entries(sections).flatMap(([name, outcome]) =>
    outcome.ok ? [] : [{ name, error: outcome.error }]
  );
  const first = failed[0];
  if (!first) return;
  throw new CliError(
    `${failed.length} of ${Object.keys(sections).length} request(s) failed: ${failed.map((f) => f.name).join(", ")}`,
    {
      code: first.error.code,
      detail: failed.map((f) => `${f.name}: ${f.error.message}`).join("\n"),
      hint: "the sections marked NOT READ above are missing, not empty"
    }
  );
}

// ── list ───────────────────────────────────────────────────────────────────

/** The live build of one listed app, or why it could not be read. */
type LiveBuildCell =
  | { state: "none" }
  | { state: "build"; build: BuildSummary }
  | { state: "failed"; error: CliError };

/** A listing row as `--json --builds` emits it. */
type ListedApp = AppRow & { live_build?: BuildSummary | null; live_build_error?: string };

export async function runAppsList(
  ctx: Context,
  flags: { org?: string; published?: boolean; draft?: boolean; builds?: boolean; json?: boolean }
): Promise<void> {
  if (flags.published && flags.draft) {
    throw usageError("--published and --draft select opposite sets", "drop one, or neither");
  }
  const conn = connect(ctx);
  const { rows: all, complete } = await listAllApps<AppRow>(conn, CUSTOMER_APPS);
  if (!complete) {
    log.warn(`stopped at ${all.length} apps — the list is TRUNCATED, the deployment has more.`);
  }

  const rows = all
    .filter((row) => !flags.org || row.org_slug === flags.org)
    .filter((row) => !flags.published || row.published_at != null)
    .filter((row) => !flags.draft || row.published_at == null)
    .sort((a, b) => appLabel(a).localeCompare(appLabel(b)));

  const cells = flags.builds ? await liveBuilds(conn, rows) : undefined;
  const listed: Listed[] = rows.map((row, i) => ({ row, cell: cells?.[i] }));

  if (flags.json) {
    printJson(
      listed.map(({ row, cell }): ListedApp => {
        if (!cell) return row;
        if (cell.state === "failed") return { ...row, live_build_error: cell.error.message };
        return { ...row, live_build: cell.state === "build" ? cell.build : null };
      })
    );
  } else if (rows.length === 0) {
    // The listing succeeded and nothing matched — every failure threw above.
    log.info(
      all.length === 0
        ? `no custom apps are visible to you on ${conn.target}`
        : `none of the ${all.length} app(s) on ${conn.target} match`
    );
  } else {
    const columns: Column<Listed>[] = [
      { header: "APP", value: ({ row }) => appLabel(row) },
      { header: "NAME", value: ({ row }) => row.name },
      { header: "STATE", value: ({ row }) => (row.published_at ? "live" : "draft") },
      { header: "PUBLISHED", value: ({ row }) => when(row.published_at) },
      { header: "LAST ACTIVE", value: ({ row }) => when(row.last_active_at) },
      { header: "SOURCE", value: ({ row }) => registeredSource(row) }
    ];
    if (cells) {
      columns.push(
        { header: "LIVE BUILD", value: buildCell((b) => b.build_id) },
        { header: "BUILT FROM", value: buildCell(provenance) },
        { header: "PUBLISHED BY", value: buildCell(publisher) },
        { header: "BUILT", value: buildCell((b) => when(b.created_at)) }
      );
    }
    process.stdout.write(`${table(listed, columns)}\n`);
    const live = rows.filter((row) => row.published_at != null).length;
    log.info(`${rows.length} app(s): ${live} live, ${rows.length - live} draft — times are UTC`);
  }

  const failed = (cells ?? []).flatMap((cell) => (cell.state === "failed" ? [cell.error] : []));
  const first = failed[0];
  if (first) {
    throw new CliError(`${failed.length} of ${rows.length} builds request(s) failed`, {
      code: first.code,
      detail: first.message,
      hint: "those rows read NOT READ (live_build_error in --json) — they are missing, not empty"
    });
  }
}

interface Listed {
  row: AppRow;
  /** Present only with `--builds`. */
  cell?: LiveBuildCell;
}

/** A `--builds` column: the live build's value, or why there is none to show. */
function buildCell(show: (build: BuildSummary) => string): (listed: Listed) => string {
  return ({ cell }) => {
    if (cell?.state === "build") return show(cell.build);
    return cell?.state === "failed" ? "NOT READ" : NONE;
  };
}

function registeredSource(row: AppRow): string {
  return row.source_unrecorded
    ? `${row.source_repo} (live build records no source)`
    : row.source_repo;
}

/** One `builds` request per app, bounded, each failure kept beside its row. */
async function liveBuilds(conn: Conn, rows: AppRow[]): Promise<LiveBuildCell[]> {
  return mapBounded(rows, BUILDS_CONCURRENCY, async (row): Promise<LiveBuildCell> => {
    const outcome = await section(fetchBuilds(conn, row.id));
    if (!outcome.ok) return { state: "failed", error: outcome.error };
    const build = liveBuild(outcome.value);
    return build ? { state: "build", build } : { state: "none" };
  });
}

// ── builds ─────────────────────────────────────────────────────────────────

export async function runAppsBuilds(ctx: Context, app: string, json: boolean): Promise<void> {
  const { conn, row } = await openApp(ctx, app);
  const history = await fetchBuilds(conn, row.id);

  if (json) {
    printJson({ app: appLabel(row), app_id: row.id, ...history });
    return;
  }
  if (history.builds.length === 0) {
    log.info(`${appLabel(row)} has no builds on ${conn.target}`);
    return;
  }
  process.stdout.write(
    `${table(history.builds, [
      { header: "BUILD", value: (b) => b.build_id },
      { header: "BUILT", value: (b) => when(b.created_at) },
      { header: "CHANNEL", value: channel },
      { header: "PUBLISHED BY", value: publisher },
      { header: "COMMIT", value: (b) => (b.commit_sha ? shortSha(b.commit_sha) : NONE) },
      { header: "BRANCH", value: (b) => b.source_branch ?? NONE },
      {
        header: "REPO",
        value: (b) => (b.source_repo ? (parseRemoteSlug(b.source_repo) ?? b.source_repo) : NONE)
      }
    ])}\n`
  );
  if (history.promoted_at) log.info(`last promote: ${lastPromote(history)} — times are UTC`);
}

/** Which channel points at a build: `live`, `draft`, both, or neither. */
function channel(build: BuildSummary): string {
  return [build.is_published && "live", build.is_draft && "draft"].filter(Boolean).join(", ");
}

// ── health, availability, errors, usage: the wire shapes ───────────────────

/** `custom_apps_health::report::HealthResponse`. */
export interface HealthReport {
  oxy_app_health: string;
  app: { id: string; org_slug: string; slug: string };
  build?: { build_id: string; published_at?: string | null } | null;
  checks: Array<{ name: string; result: string; detail?: string | null }>;
  checked_at: string;
}

/** `custom_apps_availability::AvailabilityResponse`. */
export interface Availability {
  /** `no_opinion` | `healthy` | `burning`. */
  verdict: string;
  /** `page` | `ticket`, only when burning. */
  severity?: string;
  burn_rate?: number;
  objective: number;
  windows: Array<{
    window_minutes: number;
    total: number;
    failed: number;
    failure_ratio: number | null;
  }>;
}

/** `custom_apps_logs::ClientErrorResponse`. */
interface ClientError {
  error_name: string;
  message: string;
  kind: string;
  path: string;
  build_id: string;
  occurrences: number;
  sessions: number;
  first_seen: string;
  last_seen: string;
}

/** `custom_apps_activity::ActivitySummary`. */
export interface UsageSummary {
  total_views_7d: number;
  unique_users_7d: number;
  total_events_7d: number;
  last_viewed_at: string | null;
}

interface Visitor {
  user_email: string;
  sessions: number;
  views: number;
  first_seen_at: string;
  last_seen_at: string;
  app_role: string | null;
  org_role: string | null;
}

interface EventGroup {
  event_name: string;
  count: number;
  last_fired_at: string;
}

function appPath(app: AppRow, leaf: string): string {
  return `${CUSTOMER_APPS}/${encodeURIComponent(app.org_slug)}/${encodeURIComponent(app.slug)}/${leaf}`;
}

/**
 * The deployment-integrity report for one app.
 *
 * THE 503 IS A VERDICT. This route answers 200 when every check passes and
 * 503, with the same body, when one fails — so a 503 carrying a check list is
 * the report, and treating it as "unavailable, retry" would hide exactly the
 * failure the caller asked about. Any other status, or a 503 without a check
 * list (a load balancer's page), is a request that failed.
 */
async function fetchHealth(conn: Conn, app: AppRow): Promise<HealthReport> {
  const response = await request({
    target: conn.target,
    path: appPath(app, "health"),
    method: "GET",
    bearer: conn.bearer,
    headers: conn.headers
  });
  const body = parseJson(response.body) as Partial<HealthReport> | undefined;
  const isReport =
    body != null && typeof body.oxy_app_health === "string" && Array.isArray(body.checks);
  if ((response.status === 200 || response.status === 503) && isReport) {
    return body as HealthReport;
  }
  if (response.status >= 200 && response.status < 300) {
    throw new CliError(`GET ${appPath(app, "health")} did not return a health report`, {
      code: ExitCode.FAILURE,
      detail: response.body.trim().slice(0, 500) || undefined
    });
  }
  throw errorForResponse(response);
}

function healthLine(report: HealthReport): string {
  const failed = report.checks.filter((check) => check.result !== "pass");
  if (report.oxy_app_health === "pass") return `pass (${report.checks.length} checks)`;
  return `${out.red("FAIL")} — ${failed.map((check) => check.name).join(", ") || "no check named"}`;
}

function availabilityLine(a: Availability): string {
  // The longest window is the one most likely to hold any traffic at all.
  const widest = [...a.windows].sort((x, y) => y.window_minutes - x.window_minutes)[0];
  const traffic = widest
    ? `; last ${windowLabel(widest.window_minutes)}: ${widest.total} request(s), ${widest.failed} failed`
    : "";
  const verdict = a.severity ? `${a.verdict} (${a.severity})` : a.verdict;
  return `${verdict} — objective ${percent(a.objective)}${traffic}`;
}

function percent(ratio: number): string {
  return `${Number((ratio * 100).toFixed(3))}%`;
}

function usageLine(u: UsageSummary): string {
  return (
    `${u.total_views_7d} view(s), ${u.unique_users_7d} user(s), ${u.total_events_7d} event(s)` +
    `; last viewed ${when(u.last_viewed_at)}`
  );
}

// ── show ───────────────────────────────────────────────────────────────────

export async function runAppsShow(ctx: Context, app: string, json: boolean): Promise<void> {
  const { conn, row } = await openApp(ctx, app);
  // The registry row and the builds are what this command is about; without
  // them there is nothing to show. The three below it are separate backends.
  const history = await fetchBuilds(conn, row.id);
  const [health, availability, usage] = await Promise.all([
    section(fetchHealth(conn, row)),
    section(getJson<Availability>(conn, appPath(row, "availability"))),
    section(getJson<UsageSummary>(conn, `${CUSTOMER_APPS}/${row.id}/activity/summary`))
  ]);
  const sections = { health, availability, usage };
  const live = liveBuild(history);
  const draft = draftAheadOfLive(history);

  if (json) {
    printJson({
      app: row,
      live_build: live ?? null,
      draft_build: draft ?? null,
      promoted_at: history.promoted_at,
      promoted_by_email: history.promoted_by_email,
      health: health.ok ? health.value : null,
      availability: availability.ok ? availability.value : null,
      usage: usage.ok ? usage.value : null,
      failed: failures(sections)
    });
    failIfAnyFailed(sections);
    return;
  }

  const fields: Array<[string, string]> = [
    ["app", `${appLabel(row)} — ${row.name}`],
    ["id", row.id],
    ["state", row.published_at ? `live since ${when(row.published_at)}` : "draft — not published"],
    ["url", row.url_subdomain ?? row.url],
    ["last active", when(row.last_active_at)],
    ["registered", `${row.source_repo}, branch ${row.branch}`]
  ];
  if (live) fields.push(["live build", buildLine(live)], ["", provenance(live)]);
  else fields.push(["live build", "none"]);
  if (draft) fields.push(["draft build", buildLine(draft)], ["", provenance(draft)]);
  else fields.push(["draft build", live ? "none newer than the live build" : "none"]);
  if (history.promoted_at) fields.push(["last promote", lastPromote(history)]);
  fields.push(
    ["health", health.ok ? healthLine(health.value) : notRead(health.error)],
    [
      "availability",
      availability.ok ? availabilityLine(availability.value) : notRead(availability.error)
    ],
    ["usage, 7 days", usage.ok ? usageLine(usage.value) : notRead(usage.error)]
  );
  printFields(fields);
  log.info("times are UTC");
  failIfAnyFailed(sections);
}

function buildLine(build: BuildSummary): string {
  return `${build.build_id}  built ${when(build.created_at)} by ${publisher(build)}`;
}

/**
 * When the promote (or rollback) action was last used, and by whom.
 *
 * NOT "when the live build went live". `oxyc publish --promote` puts a build
 * live without touching this field, so it can be older than the live build
 * itself — which is why it is printed as its own line and not attached to one.
 */
function lastPromote(history: { promoted_at: string | null; promoted_by_email: string | null }) {
  return `${when(history.promoted_at)} by ${history.promoted_by_email ?? "an unrecorded user"}`;
}

// ── health ─────────────────────────────────────────────────────────────────

/** One row of the fleet table — `admin::apps::fleet_health::AppHealthRow`. */
interface FleetRow {
  app_id: string;
  app_slug: string;
  app_name: string;
  org_id: string;
  org_slug: string;
  /** `down` | `degraded` | `not_measured` | `quiet` | `operational`. */
  health: string;
  reason: string | null;
  requests: number;
  failed: number;
  baseline: number | null;
  window_minutes: number;
}

const HEALTH_STATES = ["down", "degraded", "not_measured", "quiet", "operational"] as const;
type FleetSummary = Record<(typeof HEALTH_STATES)[number], number>;

interface FleetPage {
  apps: FleetRow[];
  summary: FleetSummary;
  total: number;
  has_more: boolean;
  evaluated_at: string;
  observability_configured: boolean;
}

/** The server's cap on one fleet-health page (`fleet_health::MAX_PAGE`). */
const FLEET_PAGE_SIZE = 200;
const MAX_FLEET_PAGES = 50;

/**
 * Every page of the fleet-health table as one document of the server's shape.
 *
 * The offset advances by the PAGE SIZE, not by the rows returned:
 * `needs_attention` drops rows from `apps` after the page was cut, so a page
 * can come back with no rows at all and `has_more: true`. `summary` counts
 * every app on its own page, so the pages' summaries are added up.
 */
async function fetchFleet(conn: Conn, needsAttention: boolean): Promise<FleetPage> {
  const fleet: FleetPage = {
    apps: [],
    summary: { down: 0, degraded: 0, not_measured: 0, quiet: 0, operational: 0 },
    total: 0,
    has_more: true,
    evaluated_at: "",
    observability_configured: true
  };
  for (let page = 0; page < MAX_FLEET_PAGES && fleet.has_more; page++) {
    const path =
      `${CUSTOMER_APPS}/fleet-health?limit=${FLEET_PAGE_SIZE}&offset=${page * FLEET_PAGE_SIZE}` +
      (needsAttention ? "&needs_attention=true" : "");
    const body = await getJson<FleetPage>(conn, path);
    if (!Array.isArray(body.apps) || typeof body.has_more !== "boolean") {
      throw new CliError(`GET ${path} did not return a fleet-health page`, {
        code: ExitCode.FAILURE
      });
    }
    fleet.apps.push(...body.apps);
    for (const state of HEALTH_STATES) fleet.summary[state] += body.summary?.[state] ?? 0;
    fleet.total = body.total;
    fleet.has_more = body.has_more;
    fleet.evaluated_at = body.evaluated_at;
    fleet.observability_configured = body.observability_configured;
  }
  return fleet;
}

function colourHealth(health: string): string {
  if (health === "down") return out.red(health);
  if (health === "degraded" || health === "not_measured") return out.yellow(health);
  if (health === "operational") return out.green(health);
  return out.dim(health);
}

export async function runAppsHealth(
  ctx: Context,
  app: string | undefined,
  flags: { needsAttention?: boolean; json?: boolean }
): Promise<void> {
  if (app === undefined) return fleetHealth(ctx, flags);
  if (flags.needsAttention) {
    throw usageError(
      "--needs-attention filters the fleet table",
      "drop it, or drop <app> to see every published app that needs attention"
    );
  }
  return appHealth(ctx, app, Boolean(flags.json));
}

async function fleetHealth(
  ctx: Context,
  flags: { needsAttention?: boolean; json?: boolean }
): Promise<void> {
  const conn = connect(ctx);
  const fleet = await fetchFleet(conn, Boolean(flags.needsAttention));
  if (fleet.has_more) {
    log.warn(
      `stopped after ${MAX_FLEET_PAGES} pages — the table is TRUNCATED, the fleet has more apps.`
    );
  }
  if (!fleet.observability_configured) {
    log.warn(
      `observability capture is not configured on ${conn.target} — no app's traffic is measured.`
    );
  }
  if (flags.json) {
    printJson(fleet);
    return;
  }
  if (fleet.apps.length > 0) {
    process.stdout.write(
      `${table(fleet.apps, [
        { header: "APP", value: (r) => `${r.org_slug}/${r.app_slug}` },
        { header: "NAME", value: (r) => r.app_name },
        { header: "HEALTH", value: (r) => colourHealth(r.health) },
        { header: "REASON", value: (r) => r.reason ?? "" },
        { header: "REQUESTS", value: (r) => String(r.requests), align: "right" },
        { header: "FAILED", value: (r) => String(r.failed), align: "right" },
        {
          header: "BASELINE",
          value: (r) => (r.baseline == null ? NONE : String(r.baseline)),
          align: "right"
        },
        { header: "WINDOW", value: (r) => windowLabel(r.window_minutes) }
      ])}\n`
    );
  } else if (fleet.total === 0) {
    log.info(`no published custom apps are visible to you on ${conn.target}`);
  } else {
    log.info(`none of the ${fleet.total} published app(s) needs attention`);
  }
  const s = fleet.summary;
  log.info(
    `${fleet.total} published app(s): ${s.down} down, ${s.degraded} degraded, ` +
      `${s.not_measured} not measured, ${s.quiet} quiet, ${s.operational} operational` +
      ` — evaluated ${when(fleet.evaluated_at)} UTC`
  );
}

/** How many grouped errors the table shows; `--json` carries all the server sent. */
const ERRORS_SHOWN = 10;
const ERRORS_HOURS = 24;

async function appHealth(ctx: Context, app: string, json: boolean): Promise<void> {
  const { conn, row } = await openApp(ctx, app);
  const [health, availability, errors] = await Promise.all([
    section(fetchHealth(conn, row)),
    section(getJson<Availability>(conn, appPath(row, "availability"))),
    section(
      getJson<{ errors: ClientError[] }>(conn, appPath(row, `errors?hours=${ERRORS_HOURS}`)).then(
        (body) => {
          if (Array.isArray(body.errors)) return body.errors;
          throw new CliError(`GET ${appPath(row, "errors")} did not return an error list`, {
            code: ExitCode.FAILURE
          });
        }
      )
    )
  ]);
  const sections = { health, availability, errors };

  if (json) {
    printJson({
      app: appLabel(row),
      app_id: row.id,
      health: health.ok ? health.value : null,
      availability: availability.ok ? availability.value : null,
      errors: errors.ok ? errors.value : null,
      failed: failures(sections)
    });
    failIfAnyFailed(sections);
    return;
  }

  const lines: string[] = [`${appLabel(row)} — ${row.name}`];

  lines.push(heading("Deployment integrity"));
  if (health.ok) {
    const report = health.value;
    lines.push(`${healthLine(report)}, checked ${when(report.checked_at)} UTC`);
    if (report.build) {
      lines.push(
        `serving build ${report.build.build_id}, published ${when(report.build.published_at)} UTC`
      );
    }
    lines.push(
      "",
      table(report.checks, [
        { header: "CHECK", value: (c) => c.name },
        { header: "RESULT", value: (c) => (c.result === "pass" ? c.result : out.red(c.result)) },
        { header: "DETAIL", value: (c) => c.detail ?? "" }
      ])
    );
  } else {
    lines.push(notRead(health.error));
  }

  lines.push(heading("Availability"));
  if (availability.ok) {
    const a = availability.value;
    const verdict = a.severity ? `${a.verdict} (${a.severity})` : a.verdict;
    lines.push(
      `${verdict} — objective ${percent(a.objective)}`,
      "",
      table(a.windows, [
        { header: "WINDOW", value: (w) => windowLabel(w.window_minutes) },
        { header: "REQUESTS", value: (w) => String(w.total), align: "right" },
        { header: "FAILED", value: (w) => String(w.failed), align: "right" },
        {
          header: "FAILURE RATIO",
          value: (w) => (w.failure_ratio == null ? NONE : percent(w.failure_ratio)),
          align: "right"
        }
      ])
    );
  } else {
    lines.push(notRead(availability.error));
  }

  lines.push(heading(`Browser errors, last ${ERRORS_HOURS} hours`));
  if (!errors.ok) {
    lines.push(notRead(errors.error));
  } else if (errors.value.length === 0) {
    lines.push("none recorded");
  } else {
    lines.push(
      table(errors.value.slice(0, ERRORS_SHOWN), [
        { header: "LAST SEEN", value: (e) => when(e.last_seen) },
        { header: "KIND", value: (e) => e.kind },
        { header: "ERROR", value: (e) => `${e.error_name}: ${e.message}`.slice(0, 100) },
        { header: "COUNT", value: (e) => String(e.occurrences), align: "right" },
        { header: "SESSIONS", value: (e) => String(e.sessions), align: "right" },
        { header: "PATH", value: (e) => e.path }
      ])
    );
    if (errors.value.length > ERRORS_SHOWN) {
      lines.push(`and ${errors.value.length - ERRORS_SHOWN} more — \`--json\` has all of them`);
    }
  }

  process.stdout.write(`${lines.join("\n")}\n`);
  failIfAnyFailed(sections);
}

// ── usage ──────────────────────────────────────────────────────────────────

/** The window and row cap of the visitors and events routes, sent explicitly. */
const USAGE_DAYS = 7;
const USAGE_LIMIT = 50;

export async function runAppsUsage(ctx: Context, app: string, json: boolean): Promise<void> {
  const { conn, row } = await openApp(ctx, app);
  const base = `${CUSTOMER_APPS}/${row.id}/activity`;
  const window = `days=${USAGE_DAYS}&limit=${USAGE_LIMIT}`;
  const [summary, visitors, events] = await Promise.all([
    getJson<UsageSummary>(conn, `${base}/summary`),
    getJson<{ rows: Visitor[] }>(conn, `${base}/visitors?${window}`),
    getJson<{ groups: EventGroup[] }>(conn, `${base}/events?${window}`)
  ]);
  if (!Array.isArray(visitors.rows) || !Array.isArray(events.groups)) {
    throw new CliError(`GET ${base}/… did not return the activity lists`, {
      code: ExitCode.FAILURE
    });
  }

  if (json) {
    printJson({
      app: appLabel(row),
      app_id: row.id,
      days: USAGE_DAYS,
      summary,
      visitors: visitors.rows,
      events: events.groups
    });
    return;
  }

  const lines: string[] = [
    `${appLabel(row)} — ${row.name}, last ${USAGE_DAYS} days`,
    usageLine(summary),
    heading("Visitors")
  ];
  lines.push(
    visitors.rows.length === 0
      ? "none"
      : table(visitors.rows, [
          { header: "USER", value: (v) => v.user_email },
          { header: "SESSIONS", value: (v) => String(v.sessions), align: "right" },
          { header: "VIEWS", value: (v) => String(v.views), align: "right" },
          { header: "FIRST SEEN", value: (v) => when(v.first_seen_at) },
          { header: "LAST SEEN", value: (v) => when(v.last_seen_at) },
          { header: "APP ROLE", value: (v) => v.app_role ?? NONE },
          { header: "ORG ROLE", value: (v) => v.org_role ?? NONE }
        ])
  );
  if (visitors.rows.length >= USAGE_LIMIT) {
    lines.push(`the first ${USAGE_LIMIT} only — the route caps the list`);
  }
  lines.push(heading("Events"));
  lines.push(
    events.groups.length === 0
      ? "none"
      : table(events.groups, [
          { header: "EVENT", value: (e) => e.event_name },
          { header: "COUNT", value: (e) => String(e.count), align: "right" },
          { header: "LAST FIRED", value: (e) => when(e.last_fired_at) }
        ])
  );
  if (events.groups.length >= USAGE_LIMIT) {
    lines.push(`the first ${USAGE_LIMIT} only — the route caps the list`);
  }
  process.stdout.write(`${lines.join("\n")}\n`);
}
