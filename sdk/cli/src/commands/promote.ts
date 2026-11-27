/**
 * `oxyc promote` and `oxyc rollback` — move a custom app's staging or
 * production to another build (`internal-docs/custom-app-sandboxes.md` §6;
 * environments design §5.5a).
 *
 * Two promote paths and nothing else: a sandbox's build to staging
 * (`--from dev-<handle>`), and staging's to production (the default). Each
 * verb asks the server for the plan first (`dry_run`), prints it, asks on a
 * terminal — off one it needs `--yes` and exits 8 without it — and then acts,
 * sending back the plan's `build` and `expect_current`: the server refuses
 * `409 target_moved` if the environment moved in between. A plan with
 * blockers is refused here, before any act is sent.
 *
 * `--app-env` names the environment a rollback moves, as it names an app
 * environment everywhere; `--env` stays the deployment. Staff credential only
 * (`staffCreds`): a publish token and a sandbox agent token are refused before
 * any request.
 *
 * WHERE THE APP'S ORG HOLDS PROMOTES ON A BUILD'S CHECKS, the plan carries
 * them (`checks`) and a promote to production does two more things
 * (`promote-checks.ts`): the plan lists each check, and after the move it
 * waits for production's own check and exits 9 when that failed. A rollback
 * is exempt from the checks and waits for nothing.
 */

import { parseJson, request } from "../api/request.js";
import { parseAppEnv } from "../apps/environment.js";
import { type Creds, ensureOk, type ResolvedApp, resolveApp, staffCreds } from "../apps/resolve.js";
import { isSandboxAgentToken } from "../auth/token-kind.js";
import type { Context } from "../context/resolve.js";
import {
  EVENT_LISTENERS_BROKEN,
  listenersHint,
  withListenersHint
} from "../publish/listeners-hint.js";
import * as log from "../ui/log.js";
import { out } from "../ui/tty.js";
import { CliError, ExitCode, usageError } from "../util/errors.js";
import { renderBuildChecks } from "./checks-show-render.js";
import { confirm } from "./oltp-client.js";
import {
  awaitProductionCheck,
  checksAdvice,
  PRODUCTION_WAIT,
  type ProductionWait,
  type ProductionWaitDeps,
  parseOverrideReason
} from "./promote-checks.js";
import { adviceFor, type Plan, type ProductionCheckResult, printPlan } from "./promote-plan.js";

export {
  type Blocker,
  type Plan,
  type ProductionCheckResult,
  planLines,
  printPlan
} from "./promote-plan.js";

/** A production move applies migrations inside the request: allow for them. */
const ACT_TIMEOUT_MS = 15 * 60_000;

export interface MoveFlags {
  /** promote: the source; staging when absent. */
  from?: string;
  /** promote: a build the source served; rollback: one the environment served. */
  build?: string;
  /** rollback: the environment that moves. */
  appEnv?: string;
  /** `--remove-schedule`, once per function whose schedule the move retires. */
  removeSchedules?: string[];
  /** promote: ship over production event listeners the build breaks (audited). */
  breakListeners?: boolean;
  /** promote: the reason for promoting past the build's checks (audited). */
  overrideChecks?: string;
  /** promote: do not wait for production's check after the move. */
  noWait?: boolean;
  dryRun?: boolean;
  yes?: boolean;
  json?: boolean;
}

/** Where a promote from `from` lands: a sandbox → staging, staging → production. */
export function promoteTarget(from: string): string {
  const source = parseAppEnv(from);
  if (source === "staging") return "production";
  if (source.startsWith("dev-")) return "staging";
  throw usageError(
    "production cannot be promoted",
    "promote from staging (the default) or from a dev-<handle> sandbox; to move production back, `oxyc rollback --app-env production`"
  );
}

/** The environment a rollback moves: staging or production, never a sandbox. */
export function rollbackEnvironment(appEnv: string | undefined): string {
  if (appEnv === undefined) {
    throw usageError("--app-env is required", "--app-env staging or --app-env production");
  }
  const environment = parseAppEnv(appEnv);
  if (environment.startsWith("dev-")) {
    throw usageError(
      `${environment} cannot be rolled back`,
      "a sandbox moves only by a publish to it — `oxyc publish --app-env` again"
    );
  }
  return environment;
}

interface Session {
  creds: Creds;
  app: ResolvedApp;
}

async function session(ctx: Context, app: string): Promise<Session> {
  const creds = staffCreds(ctx);
  if (isSandboxAgentToken(creds.bearer)) {
    throw usageError(
      "a sandbox agent token cannot promote or roll back",
      "a person promotes a build — sign in with `oxyc login`"
    );
  }
  return { creds, app: await resolveApp(creds, app) };
}

async function send(
  s: Session,
  path: string,
  body: Record<string, unknown>,
  timeoutMs?: number
): Promise<Plan> {
  const response = await request({
    target: s.creds.target,
    path: `/api/customer-apps/${s.app.appId}/environments/${path}`,
    method: "POST",
    body: JSON.stringify(body),
    bearer: s.creds.bearer,
    headers: s.creds.headers,
    timeoutMs
  });
  try {
    ensureOk(response, s.creds);
  } catch (cause) {
    throw withListenersHint(cause);
  }
  return parseJson(response.body) as Plan;
}

/**
 * Plan, confirm, act — the request core of both verbs, with no printing but
 * the plan and the prompt. `kind` names the route; `environment` is the one
 * that moves; `body` is the request beside `dry_run` and what the act sends
 * back.
 */
async function planThenAct(
  ctx: Context,
  appArg: string,
  move: { kind: "promote" | "rollback"; environment: string; body: Record<string, unknown> },
  flags: MoveFlags
): Promise<Moved> {
  const s = await session(ctx, appArg);
  log.info(`→ ${s.creds.target} · ${s.app.label} · ${move.environment}`);
  const path = `${encodeURIComponent(move.environment)}/${move.kind}`;
  const plan = await send(s, path, { ...move.body, dry_run: true });
  if (flags.dryRun) return { plan, session: s, app: appArg };
  // The person confirming reads the plan; `--json` keeps stdout for the result.
  if (!flags.json) printPlan(plan, process.stdout, appArg);
  else if (!flags.yes) printPlan(plan, process.stderr, appArg);
  refuseBlocked(plan, appArg);
  if (!flags.yes) {
    await confirm(
      `${move.kind === "promote" ? "Promote" : "Roll back"} ${move.environment} to build ${plan.build.build_id}?`,
      {
        verb: `a ${move.kind}`,
        why: `it moves ${move.environment} to build ${plan.build.build_id}`,
        declined: `not moved — the confirmation was declined`
      }
    );
  }
  const act = { ...move.body, expect_current: plan.expect_current };
  const pinned = move.kind === "promote" ? { build: plan.build.id } : { to: plan.build.id };
  const acted = await send(s, path, { ...act, ...pinned }, ACT_TIMEOUT_MS);
  return { plan: acted, session: s, app: appArg };
}

/** A plan, or the answer of the act, with what a follow-up request needs. */
interface Moved {
  plan: Plan;
  session: Session;
  /** The app as the caller named it, for the commands a message suggests. */
  app: string;
}

/**
 * A plan that lists blockers is not sent as an act: say why, exit 6. Each
 * blocker the checks raise is followed by what to do next. The listeners hint
 * is for any blocker that is a broken listener, wherever it is in the list.
 */
function refuseBlocked(plan: Plan, app: string): void {
  if (plan.blockers.length === 0) return;
  const first = plan.blockers[0];
  const listeners = plan.blockers.find((b) => b.code === EVENT_LISTENERS_BROKEN);
  const about = adviceFor(plan, app);
  const advice = plan.blockers
    .map((b) => checksAdvice(b.code, about))
    .filter((a): a is string => a !== undefined);
  throw new CliError(`${plan.action} refused: ${plan.blockers.map((b) => b.code).join(", ")}`, {
    code: ExitCode.REQUEST,
    hint: listeners ? listenersHint(listeners.code, listeners.message) : undefined,
    detail: plan.blockers.map((b) => `${b.code}: ${b.message}`).join("\n"),
    remedy: advice.length > 0 ? advice.join("\n") : undefined,
    serverCode: first?.code,
    serverMessage: first?.message
  });
}

async function promoteMove(ctx: Context, app: string, flags: MoveFlags): Promise<Moved> {
  const from = parseAppEnv(flags.from ?? "staging");
  const to = promoteTarget(from);
  const reason = parseOverrideReason(flags.overrideChecks);
  if (reason !== undefined && to !== "production") {
    throw usageError(
      "--override-checks goes with a promote to production",
      "a build's checks hold production only; a promote to staging is not held by them"
    );
  }
  const body: Record<string, unknown> = {
    from,
    confirm_schedule_removals: flags.removeSchedules ?? []
  };
  if (flags.build !== undefined) body.build = flags.build;
  if (flags.breakListeners) body.breakListeners = true;
  // On the plan and on the act: the plan then shows what the override clears.
  if (reason !== undefined) body.override_checks = { reason };
  return planThenAct(ctx, app, { kind: "promote", environment: to, body }, flags);
}

export async function promote(ctx: Context, app: string, flags: MoveFlags): Promise<Plan> {
  return (await promoteMove(ctx, app, flags)).plan;
}

export async function rollback(ctx: Context, app: string, flags: MoveFlags): Promise<Plan> {
  const environment = rollbackEnvironment(flags.appEnv);
  const body: Record<string, unknown> = {
    confirm_schedule_removals: flags.removeSchedules ?? []
  };
  if (flags.build !== undefined) body.to = flags.build;
  return (await planThenAct(ctx, app, { kind: "rollback", environment, body }, flags)).plan;
}

/**
 * Promote, then, where the move is one the plan says is checked, wait for
 * production's own check: print it, and exit 9 when it failed. `--no-wait`
 * and `--dry-run` wait for nothing.
 */
export async function runPromote(
  ctx: Context,
  app: string,
  flags: MoveFlags,
  waits: Partial<ProductionWaitDeps> = {}
): Promise<void> {
  const moved = await promoteMove(ctx, app, flags);
  const { plan } = moved;
  const checked = plan.moved && plan.to === "production" && plan.checks != null && !flags.noWait;
  if (!checked) {
    report(plan, flags, app);
    return;
  }
  if (!flags.json) report(plan, flags, app);
  log.info(
    `waiting for production's check of build ${plan.build.build_id} (--no-wait skips this)…`
  );
  const deps = { ...PRODUCTION_WAIT, ...waits };
  const wait = await awaitProductionCheck(moved.session.creds, plan.app_id, plan.build.id, deps);
  if (flags.json) {
    const production_check: ProductionCheckResult = {
      state: wait.outcome,
      attempt: wait.body?.attempt ?? null
    };
    process.stdout.write(`${JSON.stringify({ ...plan, production_check })}\n`);
  }
  afterProductionCheck(wait, moved, flags, deps.timeoutMs);
}

/** Say how production's check ended; a failed one is the command's failure (exit 9). */
function afterProductionCheck(
  wait: ProductionWait,
  moved: Moved,
  flags: MoveFlags,
  waitedMs: number
): void {
  const { plan, app } = moved;
  const show = `oxyc checks show ${app} --build ${plan.build.build_id} --environment production`;
  if (wait.body && wait.outcome !== "none" && !flags.json) {
    process.stdout.write(`${renderBuildChecks(moved.session.app.label, wait.body).join("\n")}\n`);
  }
  switch (wait.outcome) {
    case "passed":
      return;
    case "failed":
      throw new CliError(
        `production serves build ${plan.build.build_id}, and its check after the promote failed`,
        {
          code: ExitCode.CHECK_FAILED,
          hint: `\`${show}\` prints what it found; the people who own the app were told, and the platform rolls production back itself where the app has that on`,
          remedy: `oxyc rollback ${app} --app-env production`
        }
      );
    case "running":
      log.warn(
        `production's check had no verdict after ${Math.round(waitedMs / 1000)}s: \`${show} --wait\` reads where it is`
      );
      return;
    case "none":
      log.warn(
        "no production check was started for this build: the app may have its production check turned off"
      );
      return;
    default:
      log.warn(
        `production's check could not be read${wait.error ? ` (${wait.error.message})` : ""}: \`${show}\` reads it`
      );
  }
}

export async function runRollback(ctx: Context, app: string, flags: MoveFlags): Promise<void> {
  report(await rollback(ctx, app, flags), flags);
}

function report(plan: Plan, flags: MoveFlags, app?: string): void {
  if (flags.json) {
    process.stdout.write(`${JSON.stringify(plan)}\n`);
    return;
  }
  if (plan.dry_run) {
    printPlan(plan, process.stdout, app);
    return;
  }
  for (const warning of plan.warnings) log.warn(warning);
  const applied = plan.migrations_applied;
  const files = applied ? applied.oltp.length + applied.airhouse.length : 0;
  const migrated = files > 0 ? ` (${files} migration file(s) applied first)` : "";
  process.stdout.write(
    `${out.green(plan.to)} now serves build ${plan.build.build_id}${migrated}\n`
  );
}
