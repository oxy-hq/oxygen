/**
 * `oxyc checks show <app>` — what the promote checks found on one build.
 *
 * Every build that reaches an app's staging is checked (builds and validates,
 * its check functions, it loads on desk and phone, it follows the design
 * system; and, where the app's org holds promotes on its checks, every screen
 * opens and what changed) by a runner the platform starts (`checks-build.ts`),
 * and the results stay on the build. This reads them back:
 * one route, `GET /api/customer-apps/{id}/builds/{build}/checks[?attempt=n][&environment=…]`,
 * whose body `--json` prints as it came. `{build}` is the build's row id or its
 * `build_id`; the server resolves either within the app.
 *
 * WHICH ATTEMPT. A build has staging attempts (the default) and, once a
 * promote moved production to it, production attempts: `--environment
 * production` reads those, the one check "Production opens after promote".
 *
 * WHICH BUILD. `--build` names one. Without it, the build the environment
 * serves: for staging the row `GET …/builds` marks `is_draft`, for production
 * the one it marks `is_published`. The poll that `--wait` runs re-reads that
 * same build by its row id, so a publish landing mid-wait does not swap the
 * build under the answer.
 *
 * WHAT IT SAYS, AND DOES NOT. It prints what each check found, and the notes
 * a model left under a check, marked as a model's. Whether a promote waits on
 * a check is the promote plan's to say (`oxyc promote --dry-run`), so no line
 * here calls a check required or a build blocked. The exit code is the one
 * thing a script needs: `CHECK_FAILED` (9) when the build's checks failed,
 * `0` otherwise — passed, still running, or none run.
 *
 * Credentials are resolved as `checks run` resolves them
 * (`checks-credentials.ts`). The server answers only a caller who may open the
 * app's staging — the rows are a staging build's evidence — so a publish token
 * is answered not-found, as for a staging run it reads back. A sandbox agent
 * token is refused before any request: this route is not on its list.
 */

import { parseJson, request } from "../api/request.js";
import { type Creds, ensureOk } from "../apps/resolve.js";
import { refuseSandboxToken } from "../apps/sandbox-token.js";
import type { Context } from "../context/resolve.js";
import * as log from "../ui/log.js";
import { CliError, ExitCode, usageError } from "../util/errors.js";
import { fetchBuilds } from "./apps-client.js";
import { APPS_API, appForCredentials, resolveCredentials } from "./checks-credentials.js";
import { gating, labelOf, progress, renderBuildChecks } from "./checks-show-render.js";

export { CHECK_LABELS, renderBuildChecks } from "./checks-show-render.js";

export type CheckStatus = "queued" | "running" | "passed" | "failed" | "error" | "skipped";
export type ChecksState = "none" | "running" | "passed" | "failed";

export interface Finding {
  severity: "error" | "warning" | "info";
  message: string;
  at: string | null;
  rule: string | null;
}

/** An advisory note: a rule's or a model's, on a check or as the build's summary. */
export interface CheckNote {
  author: "rule" | "model" | string;
  model?: string | null;
  text: string;
}

/** One row of the response's `checks`, snake_case as the server sends it. */
export interface BuildCheck {
  kind: string;
  label: string | null;
  status: CheckStatus | string;
  advisory: boolean;
  summary: string | null;
  detail: { findings?: Finding[] } & Record<string, unknown>;
  /** Advisory notes on this check; an older server sends none. */
  notes?: CheckNote[] | null;
  attempt: number;
  duration_ms: number | null;
}

/** `GET …/builds/{build}/checks`, printed verbatim with `--json`. */
export interface BuildChecks {
  app_id: string;
  build: { id: string; build_id: string; created_at: string };
  attempt: number | null;
  environment: string | null;
  state: ChecksState | string;
  checks: BuildCheck[];
  /** The build's summary note, when a model wrote one. */
  summary?: CheckNote | null;
  attempts: { attempt: number; state: string; created_at: string }[];
  /** Who ran the attempt's checks; null until a runner claims it. An older server sends none. */
  runner?: { label: string; url: string | null; started_at?: string } | null;
  /** Set when the attempt got no answer in time: its open rows then read `error`. */
  stalled?: "no_runner" | "runner_stopped" | string | null;
}

export interface ChecksShowOptions {
  /** A build's `build_id` or row id; the one the environment serves when absent. */
  build?: string;
  /** `staging` (the default) or `production`: which of the build's attempts. */
  environment?: string;
  /** An earlier attempt, as typed; the latest when absent. */
  attempt?: string;
  /** Poll until no check is queued or running. */
  wait?: boolean;
  /** With `wait`: how long, in seconds. */
  timeoutSeconds: number;
  json?: boolean;
  /** How often `wait` re-reads, in ms. Tests shorten it. */
  pollMs?: number;
}

/** What `checksShowCore` read, and whether `--wait` gave up on it. */
export interface ChecksShowResult {
  label: string;
  body: BuildChecks;
  /** `--wait` reached `--timeout` with checks still queued or running. */
  timedOut: boolean;
}

const DEFAULT_POLL_MS = 5000;

/** `staging` unless it says `production`; anything else is a usage error before any request. */
export function parseChecksEnvironment(raw: string | undefined): "staging" | "production" {
  if (raw === undefined || raw === "staging") return "staging";
  if (raw === "production") return "production";
  throw usageError(
    `--environment must be staging or production, not "${raw}"`,
    "staging (the default) is the build's own checks; production is the check that follows a promote"
  );
}

/** Parse `--attempt`: a positive integer, or a usage error before any request. */
export function parseAttempt(raw: string | undefined): number | undefined {
  if (raw === undefined) return undefined;
  const n = Number(raw);
  if (!Number.isInteger(n) || n < 1) {
    throw usageError(`--attempt must be a positive whole number, not "${raw}"`, "e.g. --attempt 1");
  }
  return n;
}

/** Read the checks — and with `wait`, keep reading — without printing the answer. */
export async function checksShowCore(
  ctx: Context,
  app: string,
  opts: ChecksShowOptions
): Promise<ChecksShowResult> {
  const attempt = parseAttempt(opts.attempt);
  const environment = parseChecksEnvironment(opts.environment);
  if (opts.wait && (!Number.isFinite(opts.timeoutSeconds) || opts.timeoutSeconds <= 0)) {
    throw usageError(
      "--timeout must be a positive number of seconds",
      "e.g. --timeout 900 (the default is 600)"
    );
  }
  refuseSandboxToken(
    ctx.storedBearer(),
    "read a build's checks",
    "they are on the staff app console — a person signs in with `oxyc login`"
  );

  const target = ctx.target();
  const resolved = await resolveCredentials(ctx, target, app);
  const creds: Creds = {
    target,
    bearer: resolved.bearer,
    headers: resolved.apiKey ? { "X-API-Key": resolved.apiKey } : undefined
  };
  const { appId, label } = await appForCredentials(creds, resolved, app);
  const build = opts.build ?? (await servedBuild(creds, appId, label, environment));
  const read = (which: string) => fetchChecks(creds, appId, which, { attempt, environment });

  let body = await read(build);
  if (!opts.wait || body.state !== "running") return { label, body, timedOut: false };

  // The row id from here on: the build this answer is about, whatever staging
  // serves by the next read.
  const pinned = body.build?.id ?? build;
  const deadline = Date.now() + opts.timeoutSeconds * 1000;
  log.info(`waiting for the checks on build ${body.build?.build_id ?? build} (${progress(body)})…`);
  while (body.state === "running") {
    const left = deadline - Date.now();
    if (left <= 0) return { label, body, timedOut: true };
    await sleep(Math.min(opts.pollMs ?? DEFAULT_POLL_MS, left));
    body = await read(pinned);
  }
  return { label, body, timedOut: false };
}

export async function runChecksShow(
  ctx: Context,
  app: string,
  opts: ChecksShowOptions
): Promise<void> {
  const { label, body, timedOut } = await checksShowCore(ctx, app, opts);
  if (opts.json) {
    process.stdout.write(`${JSON.stringify(body)}\n`);
  } else {
    process.stdout.write(`${renderBuildChecks(label, body).join("\n")}\n`);
  }

  const buildId = body.build?.build_id ?? opts.build ?? "?";
  if (timedOut) {
    throw new CliError(
      `the checks on build ${buildId} were still running after ${opts.timeoutSeconds}s`,
      {
        code: ExitCode.UNAVAILABLE,
        hint: "they keep running on the server — `oxyc checks show` again reads where they are, and --timeout waits longer"
      }
    );
  }
  if (body.state === "failed") {
    const failed = gating(body).filter((c) => c.status === "failed" || c.status === "error");
    throw new CliError(
      `${failed.length} of ${gating(body).length} check(s) did not pass on build ${buildId}: ${failed.map(labelOf).join(", ")}`,
      { code: ExitCode.CHECK_FAILED }
    );
  }
}

/** The row id of the build `environment` serves, or NOT_FOUND when it serves none. */
async function servedBuild(
  creds: Creds,
  appId: string,
  label: string,
  environment: "staging" | "production"
): Promise<string> {
  const history = await fetchBuilds(creds, appId);
  const served = history.builds.find((b) =>
    environment === "production" ? b.is_published : b.is_draft
  );
  if (!served) {
    throw new CliError(`${environment} serves no build of ${label}`, {
      code: ExitCode.NOT_FOUND,
      hint: "publish one (`oxyc publish`), or name a build with --build <id> — `oxyc apps builds <app>` lists them"
    });
  }
  return served.id;
}

/** Which attempt of a build's checks to read. */
export interface ChecksQuery {
  attempt?: number;
  /** Staging's when absent. */
  environment?: "staging" | "production";
}

/** `GET …/builds/{build}/checks`: one attempt of a build's checks. */
export async function fetchChecks(
  creds: Creds,
  appId: string,
  build: string,
  which: ChecksQuery = {}
): Promise<BuildChecks> {
  const { attempt, environment } = which;
  const params = [
    ...(attempt === undefined ? [] : [`attempt=${attempt}`]),
    // Staging is the route's default: named only when it is not what is asked for.
    ...(environment === "production" ? ["environment=production"] : [])
  ];
  const query = params.length > 0 ? `?${params.join("&")}` : "";
  const path = `${APPS_API}/${appId}/builds/${encodeURIComponent(build)}/checks${query}`;
  const response = await request({
    target: creds.target,
    path,
    method: "GET",
    bearer: creds.bearer,
    headers: creds.headers
  });
  try {
    ensureOk(response, creds);
  } catch (cause) {
    if (!(cause instanceof CliError) || cause.code !== ExitCode.NOT_FOUND) throw cause;
    throw new CliError(cause.message, {
      code: cause.code,
      detail: cause.detail,
      hint: `no build "${build}" of this app${attempt === undefined ? "" : `, or no attempt ${attempt} of it`} — \`oxyc apps builds <app>\` lists its builds; a deployment older than build checks answers this way too`
    });
  }
  const parsed = parseJson(response.body) as Partial<BuildChecks> | undefined;
  if (!parsed || typeof parsed.state !== "string" || !Array.isArray(parsed.checks)) {
    throw new CliError(`GET ${path} did not return a build's checks`, {
      code: ExitCode.FAILURE,
      detail: response.body.trim().slice(0, 500) || undefined,
      hint: "the deployment may predate build checks — `oxyc routes customer-apps` lists what it mounts"
    });
  }
  // A deployment that knows no production attempts ignores the parameter and answers
  // staging's rows: never print those as production's.
  const asked = environment ?? "staging";
  const answered = parsed.environment || "staging";
  if (answered !== asked) {
    throw new CliError(
      `the platform answered a ${answered} attempt, not the ${asked} one this asked for`,
      {
        code: ExitCode.FAILURE,
        hint: "the deployment may predate production checks — `oxyc routes customer-apps` lists what it mounts"
      }
    );
  }
  return parsed as BuildChecks;
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
