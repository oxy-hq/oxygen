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
import { confirm } from "./oltp-client.js";

/** A production move applies migrations inside the request: allow for them. */
const ACT_TIMEOUT_MS = 15 * 60_000;

export interface Blocker {
  code: string;
  message: string;
}

interface Cadence {
  function: string;
  cron: string;
  timezone: string;
}

/** The server's plan, printed verbatim with `--json` (snake_case). */
export interface Plan {
  action: "promote" | "rollback";
  app_id: string;
  from: string | null;
  to: string;
  build: {
    id: string;
    build_id: string;
    commit_sha: string | null;
    source_branch: string | null;
    source_repo: string | null;
    published_by: string | null;
    created_at: string;
    published_to: string | null;
  };
  current: { id: string; build_id: string } | null;
  expect_current: string | null;
  served_since: string | null;
  served_for_seconds: number | null;
  functions: { added: string[]; removed: string[]; changed: string[] };
  schedules: {
    added: Cadence[];
    removed: Cadence[];
    changed: { function: string; from: Cadence; to: Cadence }[];
  } | null;
  migrations: { when: "before_move" | "queued_after"; oltp: string[]; airhouse: string[] };
  required_secrets_unset: string[];
  blockers: Blocker[];
  warnings: string[];
  dry_run: boolean;
  moved: boolean;
  migrations_applied: { oltp: string[]; airhouse: string[] } | null;
}

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
): Promise<Plan> {
  const s = await session(ctx, appArg);
  log.info(`→ ${s.creds.target} · ${s.app.label} · ${move.environment}`);
  const path = `${encodeURIComponent(move.environment)}/${move.kind}`;
  const plan = await send(s, path, { ...move.body, dry_run: true });
  if (flags.dryRun) return plan;
  // The person confirming reads the plan; `--json` keeps stdout for the result.
  if (!flags.json) printPlan(plan);
  else if (!flags.yes) printPlan(plan, process.stderr);
  refuseBlocked(plan);
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
  return send(s, path, { ...act, ...pinned }, ACT_TIMEOUT_MS);
}

/**
 * A plan that lists blockers is not sent as an act: say why, exit 6. The
 * listeners hint is for any blocker that is a broken listener, wherever it is
 * in the list.
 */
function refuseBlocked(plan: Plan): void {
  if (plan.blockers.length === 0) return;
  const first = plan.blockers[0];
  const listeners = plan.blockers.find((b) => b.code === EVENT_LISTENERS_BROKEN);
  throw new CliError(`${plan.action} refused: ${plan.blockers.map((b) => b.code).join(", ")}`, {
    code: ExitCode.REQUEST,
    hint: listeners ? listenersHint(listeners.code, listeners.message) : undefined,
    detail: plan.blockers.map((b) => `${b.code}: ${b.message}`).join("\n"),
    serverCode: first?.code,
    serverMessage: first?.message
  });
}

export async function promote(ctx: Context, app: string, flags: MoveFlags): Promise<Plan> {
  const from = parseAppEnv(flags.from ?? "staging");
  const to = promoteTarget(from);
  const body: Record<string, unknown> = {
    from,
    confirm_schedule_removals: flags.removeSchedules ?? []
  };
  if (flags.build !== undefined) body.build = flags.build;
  if (flags.breakListeners) body.breakListeners = true;
  return planThenAct(ctx, app, { kind: "promote", environment: to, body }, flags);
}

export async function rollback(ctx: Context, app: string, flags: MoveFlags): Promise<Plan> {
  const environment = rollbackEnvironment(flags.appEnv);
  const body: Record<string, unknown> = {
    confirm_schedule_removals: flags.removeSchedules ?? []
  };
  if (flags.build !== undefined) body.to = flags.build;
  return planThenAct(ctx, app, { kind: "rollback", environment, body }, flags);
}

export async function runPromote(ctx: Context, app: string, flags: MoveFlags): Promise<void> {
  report(await promote(ctx, app, flags), flags);
}

export async function runRollback(ctx: Context, app: string, flags: MoveFlags): Promise<void> {
  report(await rollback(ctx, app, flags), flags);
}

function report(plan: Plan, flags: MoveFlags): void {
  if (flags.json) {
    process.stdout.write(`${JSON.stringify(plan)}\n`);
    return;
  }
  if (plan.dry_run) {
    printPlan(plan);
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

/** The plan, for a person deciding whether to go ahead. */
export function printPlan(
  plan: Plan,
  stream: { write: (chunk: string) => unknown } = process.stdout
): void {
  for (const line of planLines(plan)) stream.write(`${line}\n`);
}

export function planLines(plan: Plan): string[] {
  const b = plan.build;
  const source = plan.from ?? plan.to;
  const lines = [
    `${plan.action} ${plan.from ? `${plan.from} → ` : ""}${plan.to}: build ${b.build_id}`,
    `  ${[b.commit_sha && `commit ${b.commit_sha.slice(0, 12)}`, b.source_branch && `on ${b.source_branch}`, b.published_by && `published by ${b.published_by}`, `created ${b.created_at}`].filter(Boolean).join(" · ")}`,
    `  ${plan.to} serves ${plan.current ? plan.current.build_id : "nothing"} now`
  ];
  if (plan.served_for_seconds !== null) {
    lines.push(`  ${source} has served it for ${duration(plan.served_for_seconds)}`);
  }
  const f = plan.functions;
  lines.push(`  functions: ${changes(f.added, f.removed, f.changed)}`);
  if (plan.schedules) {
    const s = plan.schedules;
    const added = s.added.map((c) => `${c.function} (${c.cron})`);
    const removed = s.removed.map((c) => c.function);
    const changed = s.changed.map((c) => `${c.function} (${c.from.cron} → ${c.to.cron})`);
    lines.push(`  schedules: ${changes(added, removed, changed)}`);
  }
  const m = plan.migrations;
  const when = m.when === "before_move" ? "applied before the move" : "queued after the move";
  lines.push(`  migrations (${when}): ${files(m.oltp, "OLTP")}; ${files(m.airhouse, "Airhouse")}`);
  if (plan.required_secrets_unset.length > 0) {
    lines.push(`  required secrets unset in production: ${plan.required_secrets_unset.join(", ")}`);
  }
  for (const w of plan.warnings) lines.push(`  ${out.yellow("warning")} ${w}`);
  for (const blocker of plan.blockers) {
    lines.push(`  ${out.red("blocked")} ${blocker.code}: ${blocker.message}`);
  }
  return lines;
}

function changes(added: string[], removed: string[], changed: string[]): string {
  const parts = [
    ...added.map((n) => `+${n}`),
    ...removed.map((n) => `-${n}`),
    ...changed.map((n) => `~${n}`)
  ];
  return parts.length > 0 ? parts.join(" ") : "no change";
}

function files(names: string[], store: string): string {
  return names.length > 0 ? `${store} ${names.join(", ")}` : `no ${store} file`;
}

function duration(seconds: number): string {
  if (seconds < 120) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 120) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  return hours < 48 ? `${hours}h ${minutes % 60}m` : `${Math.floor(hours / 24)}d ${hours % 24}h`;
}
