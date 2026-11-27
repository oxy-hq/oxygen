/**
 * `oxyc checks run <app> --build <build> --attempt <n> --report` — run one
 * build's promote checks and write their verdicts to the platform.
 *
 * Every build that reaches an app's staging gets its checks. The platform
 * keeps the rows and the console; a runner drives them, with this command.
 * It is what the platform's GitHub Actions workflow runs once per attempt, and
 * a person may run the same thing by hand.
 *
 *   validation      the platform's, written when the attempt is claimed
 *   functions       the platform's: it runs the build's `"check": true`
 *                   functions at the claim, and this command asks until they
 *                   have a verdict
 *   loads           this command's: headless Chromium opens staging at desk
 *                   and phone width (`checks/loads-*.ts`)
 *   design_system   this command's: the SDK floor and the SDK's theme guard
 *                   on the build's CSS (`checks/design-system.ts`)
 *   smoke           this command's, where the app's org holds promotes on
 *                   its checks: every screen at both widths
 *   changes         this command's, there too: each screen pictured on
 *                   staging and on production, and compared
 *                   (`checks/screens-run.ts`)
 *
 * `--environment production` runs the ONE check of a production attempt, the
 * one the platform queues when a promote moves production:
 *
 *   production_smoke   every screen of production, once it serves the build,
 *                      against what it showed before (`checks/production-run.ts`)
 *
 * Only what the claim leaves open is run. Each check this command owns is
 * reported as soon as it has a verdict, so the console fills in as they
 * finish. A check that throws is reported as `error` with why: a row is never
 * left open by a crash here.
 *
 * `--report` IS REQUIRED. This mode writes to the platform, and a flag that
 * says so keeps `--build` from reading as a way to look at a build's checks
 * (`oxyc checks show --build` is that).
 *
 * THE APP. A UUID is used as it is, with no lookup: the platform's checker
 * may not list apps. `<org>/<app>` is resolved by the app listing, which
 * needs a person's credential.
 *
 * EXIT CODES. `0` every check has a verdict and none failed (or the attempt
 * was already closed). `9` a check's verdict is `failed` or `error`. A
 * verdict that could not be written, or a refused claim, exits with that
 * request's own code (4, 5, 6, 7). `7` when the build's check functions had
 * no verdict in time.
 */

import { type Creds, resolveApp, UUID_RE } from "../apps/resolve.js";
import { refuseSandboxToken } from "../apps/sandbox-token.js";
import {
  type AttemptRun,
  type BuildCheckLine,
  type BuildChecksDeps,
  DEFAULT_DEPS,
  driveAttempt
} from "../checks/attempt-run.js";
import {
  type CheckEnvironment,
  claimAttempt,
  type PromoteApi,
  type Runner
} from "../checks/promote-api.js";
import { runnerIdentity } from "../checks/runner-identity.js";
import { type Context, notAuthenticated } from "../context/resolve.js";
import * as log from "../ui/log.js";
import { err } from "../ui/tty.js";
import { CliError, ExitCode, usageError } from "../util/errors.js";
import { CHECK_LABELS, parseAttempt } from "./checks-show.js";

export type { BuildCheckLine, BuildChecksDeps } from "../checks/attempt-run.js";

export interface BuildChecksOptions {
  /** The build's row id or its `build_id`. Absent only when a flag of this mode was passed without it. */
  build?: string;
  /** The attempt, as typed; the newest when absent. */
  attempt?: string;
  /** Write the verdicts to the platform. Required. */
  report?: boolean;
  /** `staging` (the default), or `production`: the attempt a promote queued. */
  environment?: string;
  json?: boolean;
  /** `checks run`'s other mode's flags, refused here by name. */
  appEnv?: string;
  browser?: boolean;
  expectSelector?: string;
}

/** The `--json` summary: one object on stdout. */
export interface BuildChecksSummary {
  app: string;
  appId: string;
  build: { id: string; build_id: string } | null;
  attempt: number | null;
  environment: string | null;
  runner: Runner;
  /** Every check of the attempt already had a verdict: nothing was run. */
  closed: boolean;
  /** In the console's order (functions, loads, design_system, smoke, changes). Only what this run drove. */
  checks: BuildCheckLine[];
  /** The platform's roll-up after the last verdict written; null when none was. */
  state: string | null;
  /** The kinds still without a verdict, as the platform last said. */
  open: string[];
}

export interface BuildChecksResult {
  summary: BuildChecksSummary;
  /** Why the run is not a clean one, already carrying its exit code. */
  failure?: CliError;
}

const ORDER = ["functions", "loads", "design_system", "smoke", "changes", "production_smoke"];

/** `--environment`: staging unless it says production; anything else is refused before any request. */
export function parseCheckEnvironment(raw: string | undefined): CheckEnvironment {
  if (raw === undefined || raw === "staging") return "staging";
  if (raw === "production") return "production";
  throw usageError(
    `--environment must be staging or production, not "${raw}"`,
    "staging (the default) is the build's own checks; production is the check a promote queues"
  );
}

interface Parsed {
  build: string;
  attempt: number | null;
  environment: CheckEnvironment;
}

/** Refuse a wrong invocation before any request. Returns the build, the attempt and the environment. */
function parseOptions(opts: BuildChecksOptions): Parsed {
  if (!opts.build) {
    throw usageError(
      "--report, --attempt and --environment need --build <build>",
      "oxyc checks run <app> --build <build> --attempt <n> --report runs a build's promote checks"
    );
  }
  if (opts.appEnv !== undefined || opts.browser || opts.expectSelector !== undefined) {
    throw usageError(
      "--app-env, --browser and --expect-selector do not go with --build",
      "a build's promote checks run on staging, or with --environment production on production, at desk and phone width"
    );
  }
  const attempt = parseAttempt(opts.attempt) ?? null;
  const environment = parseCheckEnvironment(opts.environment);
  if (!opts.report) {
    throw usageError(
      "--build runs a build's promote checks and writes their verdicts to the platform: pass --report to say so",
      `to read a build's checks without running any: oxyc checks show <app> --build ${opts.build}`
    );
  }
  return { build: opts.build, attempt, environment };
}

/** The platform's checker (an API key), or a person: the stored bearer. Nothing is minted. */
function runnerCreds(ctx: Context): Creds {
  const bearer = ctx.storedBearer();
  refuseSandboxToken(
    bearer,
    "run a build's promote checks",
    "they run as the platform's checker (its token in OXY_API_KEY) or as staff (`oxyc login`)"
  );
  if (bearer) return { target: ctx.target(), bearer };
  const apiKey = ctx.apiKey();
  if (apiKey) return { target: ctx.target(), headers: { "X-API-Key": apiKey } };
  throw notAuthenticated(ctx.target(), ctx.flags);
}

function labelOf(kind: string): string {
  return CHECK_LABELS[kind] ?? kind;
}

/** Why the run is not clean, by what a caller should do about it; `undefined` when it is. */
function failureOf(
  run: AttemptRun,
  summary: BuildChecksSummary,
  deps: BuildChecksDeps
): CliError | undefined {
  // A verdict that was not written is a broken run, whatever the verdicts say.
  if (run.broke) return run.broke;
  const build = summary.build?.build_id ?? "?";
  const failed = summary.checks.filter((c) => c.status === "failed" || c.status === "error");
  if (failed.length > 0) {
    return new CliError(
      `${failed.length} of ${summary.checks.length} check(s) did not pass on build ${build}: ${failed.map((c) => labelOf(c.kind)).join(", ")}`,
      { code: ExitCode.CHECK_FAILED }
    );
  }
  if (summary.state === "failed") {
    return new CliError(`the checks of build ${build} failed on the platform`, {
      code: ExitCode.CHECK_FAILED,
      hint: `oxyc checks show ${summary.appId} --build ${build} prints every check`
    });
  }
  if (run.unsettled) {
    return new CliError(
      `the check functions of build ${build} had no verdict after ${Math.round(deps.settleTimeoutMs / 1000)}s`,
      {
        code: ExitCode.UNAVAILABLE,
        hint: `they carry on on the platform — oxyc checks show ${summary.appId} --build ${build} --wait reads where they are`
      }
    );
  }
  return undefined;
}

/** Claim the attempt and drive what is open. No printing; `onLine` is called as each check lands. */
export async function buildChecksCore(
  ctx: Context,
  app: string,
  opts: BuildChecksOptions,
  overrides: Partial<BuildChecksDeps> = {},
  onLine?: (line: BuildCheckLine) => void
): Promise<BuildChecksResult> {
  const deps = { ...DEFAULT_DEPS, ...overrides };
  const { build, attempt, environment } = parseOptions(opts);
  const creds = runnerCreds(ctx);
  // A UUID is the id: no lookup, which the platform's checker could not make.
  const named = UUID_RE.test(app) ? { appId: app, label: app } : await resolveApp(creds, app);
  const api: PromoteApi = {
    creds,
    appId: named.appId,
    build,
    environment,
    backoffMs: deps.backoffMs,
    sleep: deps.sleep
  };
  const runner = runnerIdentity(deps.env);
  const summary: BuildChecksSummary = {
    app: named.label,
    appId: named.appId,
    build: null,
    attempt,
    environment,
    runner,
    closed: false,
    checks: [],
    state: null,
    open: []
  };

  const claim = await claimAttempt(api, attempt, runner);
  if (claim === "closed") return { summary: { ...summary, closed: true } };
  // A deployment that knows no production attempts answers staging's: that is not what was asked for.
  if ((claim.environment || "staging") !== environment) {
    throw new CliError(
      `the platform answered a ${claim.environment} attempt, not the ${environment} one this run asked for`,
      {
        code: ExitCode.FAILURE,
        hint: "the deployment may predate production checks — `oxyc routes customer-apps` lists what it mounts"
      }
    );
  }

  summary.app = `${claim.app.org_slug}/${claim.app.slug}`;
  summary.build = { id: claim.build.id, build_id: claim.build.build_id };
  summary.attempt = claim.attempt;
  summary.environment = claim.environment;
  log.info(
    `checking ${summary.app} · build ${claim.build.build_id} · ${claim.environment} · attempt ${claim.attempt} (${runner.label})`
  );

  const run = await driveAttempt({ api, claim, creds, deps, onLine });

  summary.checks = [...run.lines].sort((a, b) => ORDER.indexOf(a.kind) - ORDER.indexOf(b.kind));
  summary.state = run.state;
  // A verdict the platform did not take leaves its check open there.
  const settled = new Set(
    run.lines.filter((l) => l.status !== "running" && l.reported !== false).map((l) => l.kind)
  );
  summary.open = run.open.filter((kind) => !settled.has(kind));
  return { summary, failure: failureOf(run, summary, deps) };
}

function printLine(line: BuildCheckLine): void {
  if (process.env.OXYC_QUIET) return;
  const mark =
    line.status === "passed"
      ? err.green("✓")
      : line.status === "failed"
        ? err.red("✗")
        : line.status === "skipped"
          ? err.dim("-")
          : line.status === "running"
            ? err.cyan("…")
            : err.yellow("!");
  const unwritten = line.reported === false ? err.yellow(" (verdict not written)") : "";
  process.stderr.write(
    `${mark} ${labelOf(line.kind)}  ${(line.summary ?? line.status).slice(0, 200)}${unwritten}\n`
  );
}

export async function runBuildChecks(
  ctx: Context,
  app: string,
  opts: BuildChecksOptions,
  overrides: Partial<BuildChecksDeps> = {}
): Promise<void> {
  const { summary, failure } = await buildChecksCore(
    ctx,
    app,
    opts,
    overrides,
    opts.json ? undefined : printLine
  );
  if (opts.json) process.stdout.write(`${JSON.stringify(summary)}\n`);
  if (summary.closed) {
    log.info(
      `every check of ${summary.app} build ${opts.build} already has a verdict: nothing is left to check`
    );
    return;
  }
  if (failure) throw failure;
  if (summary.checks.length === 0) log.info("nothing was left for a runner to check");
}
