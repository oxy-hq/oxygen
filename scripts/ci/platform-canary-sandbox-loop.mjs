#!/usr/bin/env node
/**
 * The sandbox-environments integration loop (plan §5.1): nine steps, driven
 * entirely through `oxyc`, that create two `dev-<handle>` sandboxes of the
 * platform canary, publish a different build to each, configure each one's
 * `CANARY_STEPS` independently, run checks in both, call a function in one,
 * confirm production is untouched, then delete both and confirm what should
 * and should not survive the teardown.
 *
 * Called from `platform-canary-checkpoint.mjs` behind `--sandbox-loop`, after
 * its second green `checks run`, reusing the checkpoint's staged canary (as
 * the publish cwd), its staff `dev-login` token as `OXY_TOKEN`, and the
 * workspace build of `oxyc`. Can also run standalone against an
 * already-published canary — see `main()` below — which is mainly useful for
 * iterating on this script itself.
 *
 * Every assertion is on a JSON field or an exit code, never on stderr text —
 * `oxyc`'s exit-code contract (`oxyc exit-codes`) is what an agent branches
 * on, and stderr is free-text progress, not a contract. Each step prints one
 * line, `ok <n> <what>`; a failure prints the step, the command, its exit
 * code and the stdout/stderr it got, then this still attempts to delete both
 * sandboxes before exiting non-zero — so a failed run never leaves `dev-loop-a`
 * / `dev-loop-b` behind to block the next one. The same cleanup runs BEFORE
 * step 1, unconditionally, which is what makes a re-run after an aborted one
 * safe.
 */

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import {
  ALL_STEPS_SOURCE,
  deriveCiSteps,
  OMITTED_IN_CI,
  parseAllSteps
} from "./platform-canary-checkpoint.mjs";

const REPO = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const OXYC = join(REPO, "sdk", "cli", "dist", "main.mjs");

export const SANDBOX_A = "dev-loop-a";
export const SANDBOX_B = "dev-loop-b";

/**
 * The CANARY_STEPS each sandbox gets (plan §5.1 step 4), checked against
 * `STEP_OPS` in `customer-apps/examples/platform-canary/functions/steps.ts`
 * and the non-production table (`env_policy::non_production`), for an org
 * with no OLTP staging branch and no `nonProduction.destinations` — this
 * canary's org:
 *
 *   sql_read            -> query                         a production read
 *   org_read            -> org.places/people/assignments a production read
 *   storage_roundtrip   -> storage.* in the sandbox's own silo, plus a
 *                          presigned PUT sent through ctx.fetch
 *
 * `sql_read` and `org_read` are never held, so sandbox A's check passes with
 * nothing held. `storage_roundtrip` is in the default list by decision, and
 * is the step to omit (`--omit-steps`, below) where it cannot run: its
 * presigned PUT goes through `ctx.fetch`, which refuses a loopback host (the
 * reason the checkpoint's own `OMITTED_IN_CI` gives) and which the
 * non-production table holds for any mutating method (`Fetch => Hold`). The
 * second reason is read from the policy, not yet seen on a server: if it
 * holds, the step fails in every sandbox, on a dev box too, until the policy
 * lets a sandbox PUT to a URL its own `ctx.storage.getUploadUrl` minted.
 *
 *   warehouse_insert    -> warehouse.exec, then warehouse.insert
 *
 * The canary's `oxy-app.json` declares no `"nonProduction": { "destinations":
 * {...} }` for `canary_warehouse`, so the write is HELD. A held call throws
 * (`HeldInStaging: ctx.warehouse.exec was not performed…`), and the step's
 * first call is the `CREATE TABLE` in `ensureTable` — so the op the held list
 * names is `warehouse.exec`; `warehouse.insert` is never reached.
 */
export const STEPS_A = ["sql_read", "org_read", "storage_roundtrip"];
export const STEPS_B = ["warehouse_insert"];
/** The canary step sandbox B runs, and the op of it that is held: its first. */
export const HELD_STEP_B = "warehouse_insert";
export const HELD_OP_B = "warehouse.exec";

/**
 * The key an invocation row carries its id under, on both listings
 * (`InvocationSummary::id` in `admin/apps/invocations.rs`). The held route
 * and the run detail name the same id `invocation_id`; a row does not.
 */
export const INVOCATION_ROW_ID = "id";

/**
 * Sandbox A's steps with `omit` taken out, in `STEPS_A`'s order. An omission
 * that is not one of A's steps, or that leaves A nothing to run, throws: a
 * typo must not silently run the full list, and an empty `CANARY_STEPS` means
 * "every step" to the canary.
 */
export function sandboxASteps(omit = []) {
  for (const name of omit) {
    if (!STEPS_A.includes(name)) {
      throw new Error(`cannot omit ${name}: sandbox A runs ${STEPS_A.join(", ")}`);
    }
  }
  const steps = STEPS_A.filter((step) => !omit.includes(step));
  if (steps.length === 0) throw new Error("every step of sandbox A is omitted");
  return steps;
}

/** `--omit-steps a,b` / `CANARY_SANDBOX_LOOP_OMIT`: names, trimmed, blanks dropped. */
export function parseOmit(csv) {
  return (csv ?? "")
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean);
}

// ── oxyc, captured ──────────────────────────────────────────────────────────

/** Shell `node <oxyc> <args>`, stdout and stderr captured separately (never inherited). */
function oxyc(ctx, args, { cwd } = {}) {
  const cmd = ["node", ctx.oxyc, ...args];
  const r = spawnSync(cmd[0], cmd.slice(1), {
    cwd: cwd ?? ctx.appDir,
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
    env: { ...process.env, OXY_TOKEN: ctx.token, OXY_CREDENTIALS_PATH: ctx.credentialsPath }
  });
  return { cmd, status: r.status ?? -1, stdout: r.stdout ?? "", stderr: r.stderr ?? "" };
}

class StepFailure extends Error {
  constructor(detail, call) {
    super(detail);
    this.call = call;
  }
}

function assertStatus(call, expected, label) {
  if (call.status !== expected) {
    throw new StepFailure(`${label}: expected exit ${expected}, got ${call.status}`, call);
  }
}

function assertJson(call, cond, label) {
  if (!cond) throw new StepFailure(label, call);
}

function parseJsonOrThrow(call, label) {
  try {
    return JSON.parse(call.stdout);
  } catch {
    throw new StepFailure(`${label}: stdout is not JSON`, call);
  }
}

function arraysEqual(a, b) {
  return (
    Array.isArray(a) && Array.isArray(b) && a.length === b.length && a.every((x, i) => x === b[i])
  );
}

/**
 * When sandbox A's check failed at a step that can be omitted here, the line
 * that says so and how — or `undefined`. Read from the report's `error`
 * field, the canary's own `canary step <name> failed: …`.
 */
export function omitHint(call) {
  if (call.status !== 9) return undefined;
  let report;
  try {
    report = JSON.parse(call.stdout);
  } catch {
    return undefined;
  }
  const error = report?.checks?.find((c) => c.passed === false)?.error ?? "";
  const step = /canary step ([a-z_]+) failed/.exec(error)?.[1];
  if (step !== "storage_roundtrip") return undefined;
  return (
    "dev-loop-a's check failed at storage_roundtrip: its presigned PUT goes through " +
    "ctx.fetch, which refuses a loopback object store and holds a mutating request outside " +
    "production. Re-run with --sandbox-loop-omit storage_roundtrip " +
    "(or CANARY_SANDBOX_LOOP_OMIT=storage_roundtrip)"
  );
}

/** `csv`'s names, in `allSteps`' canonical order — the order the canary itself runs them in. */
export function expectedOrder(csv, allSteps) {
  const named = new Set(
    csv
      .split(",")
      .map((s) => s.trim())
      .filter(Boolean)
  );
  return allSteps.filter((s) => named.has(s));
}

// ── cleanup: pre-flight (re-run safety) and on-failure ──────────────────────

function deleteSandbox(ctx, name) {
  return oxyc(ctx, [
    "env",
    "delete",
    ctx.app,
    name,
    "--yes",
    "--wait",
    "--target",
    ctx.target,
    "--json"
  ]);
}

/** Exit 0 (deleted) or 5 (never existed) are both fine; anything else is a loud warning, not fatal. */
async function cleanupSandbox(ctx, name) {
  const call = deleteSandbox(ctx, name);
  if (call.status === 0 || call.status === 5) return;
  process.stderr.write(
    `  cleanup: deleting ${name} exited ${call.status} (continuing)\n` +
      `    stderr: ${call.stderr.slice(0, 500)}\n`
  );
}

async function cleanupBoth(ctx, label) {
  process.stdout.write(`-- cleanup (${label}) --\n`);
  await cleanupSandbox(ctx, SANDBOX_A);
  await cleanupSandbox(ctx, SANDBOX_B);
}

// ── the nine steps ───────────────────────────────────────────────────────────

async function step1(ctx, state) {
  const call = oxyc(ctx, ["env", "list", ctx.app, "--target", ctx.target, "--json"]);
  assertStatus(call, 0, "env list");
  const envs = parseJsonOrThrow(call, "env list").environments ?? [];
  assertJson(
    call,
    envs.some((e) => e.kind === "production"),
    "no production environment listed"
  );
  state.baseline = JSON.stringify(envs.filter((e) => e.kind !== "dev"));
}

async function step2(ctx) {
  for (const name of [SANDBOX_A, SANDBOX_B]) {
    const call = oxyc(ctx, ["env", "create", ctx.app, name, "--target", ctx.target, "--json"]);
    assertStatus(call, 0, `create ${name}`);
    const env = parseJsonOrThrow(call, `create ${name}`);
    assertJson(call, env.name === name, `create ${name}: response named ${env.name}`);
    assertJson(
      call,
      env.build_id === null,
      `create ${name}: build_id was ${env.build_id}, not null`
    );
  }
  const dup = oxyc(ctx, ["env", "create", ctx.app, SANDBOX_A, "--target", ctx.target, "--json"]);
  assertStatus(dup, 6, "duplicate create of dev-loop-a");
}

async function publishSandbox(ctx, state, name, buildId) {
  const call = oxyc(
    ctx,
    [
      "publish",
      "--app-env",
      name,
      "--build-id",
      buildId,
      "--target",
      ctx.target,
      "--org",
      ctx.org,
      "--project",
      ctx.project,
      "--json"
    ],
    { cwd: ctx.appDir }
  );
  assertStatus(call, 0, `publish to ${name}`);
  const result = parseJsonOrThrow(call, `publish to ${name}`);
  assertJson(
    call,
    result.channel === "sandbox",
    `publish to ${name}: channel was ${result.channel}`
  );
  assertJson(
    call,
    result.environment === name,
    `publish to ${name}: environment was ${result.environment}`
  );
  assertJson(
    call,
    result.build_id === buildId,
    `publish to ${name}: build_id was ${result.build_id}`
  );
  state.buildIds[name] = buildId;

  const show = oxyc(ctx, ["env", "show", ctx.app, name, "--target", ctx.target, "--json"]);
  assertStatus(show, 0, `env show ${name}`);
  const shown = parseJsonOrThrow(show, `env show ${name}`);
  assertJson(show, shown.build_id === buildId, `env show ${name}: build_id was ${shown.build_id}`);
}

async function step3(ctx, state) {
  state.buildIds = {};
  const stamp = Date.now();
  await publishSandbox(ctx, state, SANDBOX_A, `sbx-loop-a-${stamp}`);
  await publishSandbox(ctx, state, SANDBOX_B, `sbx-loop-b-${stamp}`);

  const after = oxyc(ctx, ["env", "list", ctx.app, "--target", ctx.target, "--json"]);
  assertStatus(after, 0, "env list after sandbox publishes");
  const envs = parseJsonOrThrow(after, "env list after sandbox publishes").environments ?? [];
  const now = JSON.stringify(envs.filter((e) => e.kind !== "dev"));
  assertJson(after, now === state.baseline, "production/staging changed after a sandbox publish");
}

async function step4(ctx, state) {
  const path = `/api/customer-apps/${ctx.appId}/secrets`;
  for (const { name, steps } of [
    { name: SANDBOX_A, steps: state.stepsA },
    { name: SANDBOX_B, steps: STEPS_B }
  ]) {
    const call = oxyc(ctx, [
      "api",
      "-X",
      "POST",
      path,
      "-f",
      "key=CANARY_STEPS",
      "-f",
      `value=${steps.join(",")}`,
      "-f",
      `environment=${name}`,
      "--target",
      ctx.target
    ]);
    // `oxyc api` exits 0 only on a 2xx; the route answers 204, which this
    // CLI prints nothing for — exit 0 IS the "204" assertion here.
    assertStatus(call, 0, `set CANARY_STEPS for ${name}`);
  }
}

async function step5(ctx, state) {
  const call = oxyc(ctx, [
    "checks",
    "run",
    ctx.app,
    "--app-env",
    SANDBOX_A,
    "--target",
    ctx.target,
    "--timeout",
    String(ctx.checkTimeoutSeconds),
    "--json"
  ]);
  const omit = omitHint(call);
  if (omit) throw new StepFailure(omit, call);
  assertStatus(call, 0, "checks run on dev-loop-a");
  const report = parseJsonOrThrow(call, "checks run on dev-loop-a");
  assertJson(
    call,
    report.environment === SANDBOX_A,
    `report.environment was ${report.environment}`
  );
  assertJson(
    call,
    report.checks?.length === 1 && report.checks[0].passed === true,
    `dev-loop-a's check did not pass: ${JSON.stringify(report.checks)}`
  );
  const check = report.checks[0];
  assertJson(call, Boolean(check.invocationId), "no invocationId on the passed check");
  state.invocationA = check.invocationId;

  const detail = oxyc(ctx, [
    "api",
    `/api/admin/apps/${ctx.appId}/function-runs/${check.runId}?environment=${SANDBOX_A}`,
    "--target",
    ctx.target
  ]);
  assertStatus(detail, 0, "function-run detail for dev-loop-a");
  const run = parseJsonOrThrow(detail, "function-run detail for dev-loop-a");
  const answer = JSON.parse(run.answer ?? "null");
  assertJson(
    detail,
    arraysEqual(answer?.steps, state.stepsA),
    `dev-loop-a ran [${answer?.steps}], expected [${state.stepsA}]`
  );

  const held = oxyc(ctx, [
    "invocations",
    "held",
    ctx.app,
    check.invocationId,
    "--target",
    ctx.target,
    "--json"
  ]);
  assertStatus(held, 0, "invocations held for dev-loop-a's check");
  const heldPayload = parseJsonOrThrow(held, "invocations held for dev-loop-a's check");
  assertJson(
    held,
    heldPayload.environment === SANDBOX_A,
    `held.environment was ${heldPayload.environment}`
  );
  assertJson(
    held,
    heldPayload.build_id === state.buildIds[SANDBOX_A],
    `held.build_id (${heldPayload.build_id}) did not match A's build (${state.buildIds[SANDBOX_A]})`
  );
  assertJson(
    held,
    (heldPayload.held ?? []).length === 0,
    `dev-loop-a held a write it should not have: ${JSON.stringify(heldPayload.held)}`
  );
}

async function step6(ctx, state) {
  const call = oxyc(ctx, [
    "checks",
    "run",
    ctx.app,
    "--app-env",
    SANDBOX_B,
    "--target",
    ctx.target,
    "--timeout",
    String(ctx.checkTimeoutSeconds),
    "--json"
  ]);
  assertStatus(call, 9, "checks run on dev-loop-b");
  const report = parseJsonOrThrow(call, "checks run on dev-loop-b");
  assertJson(
    call,
    report.environment === SANDBOX_B,
    `report.environment was ${report.environment}`
  );
  const check = report.checks?.[0];
  assertJson(
    call,
    Boolean(check) && check.passed === false,
    "dev-loop-b's check should have failed"
  );
  assertJson(
    call,
    /HeldInStaging/.test(check?.error ?? ""),
    `dev-loop-b's error did not name HeldInStaging: ${check?.error}`
  );
  assertJson(call, Boolean(check.invocationId), "no invocationId on the failed check");
  state.invocationB = check.invocationId;

  const held = oxyc(ctx, [
    "invocations",
    "held",
    ctx.app,
    check.invocationId,
    "--target",
    ctx.target,
    "--json"
  ]);
  assertStatus(held, 0, "invocations held for dev-loop-b's check");
  const heldPayload = parseJsonOrThrow(held, "invocations held for dev-loop-b's check");
  assertJson(
    held,
    (heldPayload.held ?? []).some((h) => h.op === HELD_OP_B),
    `held list did not include ${HELD_OP_B}: ${JSON.stringify(heldPayload.held)}`
  );
}

async function step7(ctx, state) {
  const call = oxyc(ctx, [
    "fn",
    "call",
    ctx.app,
    "canary",
    "--app-env",
    SANDBOX_A,
    "--target",
    ctx.target,
    "--json"
  ]);
  assertStatus(call, 0, "fn call canary on dev-loop-a");
  const result = parseJsonOrThrow(call, "fn call canary on dev-loop-a");
  assertJson(call, result.ok === true, `fn call did not succeed: ${result.error}`);
  assertJson(call, Boolean(result.invocationId), "fn call reported no invocationId");

  const list = oxyc(ctx, [
    "invocations",
    "list",
    ctx.app,
    "--app-env",
    SANDBOX_A,
    "--build",
    state.buildIds[SANDBOX_A],
    "--target",
    ctx.target,
    "--json"
  ]);
  assertStatus(list, 0, "invocations list for dev-loop-a");
  const invocations = parseJsonOrThrow(list, "invocations list for dev-loop-a").invocations ?? [];
  assertJson(
    list,
    invocations.some((i) => i[INVOCATION_ROW_ID] === result.invocationId),
    "fn call's invocation is missing from the list"
  );
  assertJson(
    list,
    invocations.every((i) => i.environment === SANDBOX_A),
    "the list leaked a row from another environment"
  );
}

async function step8(ctx) {
  const call = oxyc(ctx, [
    "checks",
    "run",
    ctx.app,
    "--target",
    ctx.target,
    "--timeout",
    String(ctx.checkTimeoutSeconds),
    "--json"
  ]);
  assertStatus(call, 0, "checks run on production");
  const report = parseJsonOrThrow(call, "checks run on production");
  assertJson(
    call,
    report.environment === undefined,
    `production report named an environment: ${report.environment}`
  );
  assertJson(
    call,
    report.checks?.length === 1 && report.checks[0].passed === true,
    `production's check did not pass: ${JSON.stringify(report.checks)}`
  );
  const check = report.checks[0];

  const detail = oxyc(ctx, [
    "api",
    `/api/admin/apps/${ctx.appId}/function-runs/${check.runId}`,
    "--target",
    ctx.target
  ]);
  assertStatus(detail, 0, "function-run detail for production");
  const run = parseJsonOrThrow(detail, "function-run detail for production");
  const answer = JSON.parse(run.answer ?? "null");
  assertJson(
    detail,
    arraysEqual(answer?.steps, ctx.productionSteps),
    `production ran [${answer?.steps}], expected [${ctx.productionSteps}]`
  );
}

async function step9(ctx, state) {
  for (const name of [SANDBOX_A, SANDBOX_B]) {
    const del = deleteSandbox(ctx, name);
    assertStatus(del, 0, `delete ${name}`);
    const result = parseJsonOrThrow(del, `delete ${name}`);
    assertJson(del, result.status === "deleted", `delete ${name}: status was ${result.status}`);

    const show = oxyc(ctx, ["env", "show", ctx.app, name, "--target", ctx.target, "--json"]);
    assertStatus(show, 5, `env show ${name} after delete`);
  }

  const listA = oxyc(ctx, [
    "invocations",
    "list",
    ctx.app,
    "--app-env",
    SANDBOX_A,
    "--target",
    ctx.target,
    "--json"
  ]);
  assertStatus(listA, 0, "invocations list for dev-loop-a after delete");
  const invocationsA =
    parseJsonOrThrow(listA, "invocations list for dev-loop-a after delete").invocations ?? [];
  assertJson(
    listA,
    invocationsA.some((i) => i[INVOCATION_ROW_ID] === state.invocationA),
    "step 5's invocation row is gone after deleting dev-loop-a"
  );

  const recreate = oxyc(ctx, [
    "env",
    "create",
    ctx.app,
    SANDBOX_A,
    "--target",
    ctx.target,
    "--json"
  ]);
  assertStatus(recreate, 0, "recreate dev-loop-a after delete");
}

/** The nine steps, as a table — one runner below drives all of them. */
export const STEPS = [
  { n: 1, what: "env list — baseline production/staging build ids", run: step1 },
  { n: 2, what: "create dev-loop-a, dev-loop-b; duplicate create refused (exit 6)", run: step2 },
  {
    n: 3,
    what: "publish a distinct build to each sandbox; production/staging untouched",
    run: step3
  },
  { n: 4, what: "CANARY_STEPS set per sandbox", run: step4 },
  { n: 5, what: "checks run on dev-loop-a: passes, exactly A's steps, nothing held", run: step5 },
  { n: 6, what: "checks run on dev-loop-b: HeldInStaging on warehouse_insert, exit 9", run: step6 },
  { n: 7, what: "fn call canary on dev-loop-a; invocation visible only there", run: step7 },
  { n: 8, what: "checks run on production: still green, production's own steps", run: step8 },
  { n: 9, what: "delete both sandboxes; history kept; the name frees up", run: step9 }
];

// ── driver ───────────────────────────────────────────────────────────────────

function truncate(s) {
  return s.length > 4000 ? `${s.slice(0, 4000)}… (truncated)` : s;
}

function reportFailure(n, what, e) {
  process.stderr.write(`\nFAIL step ${n}: ${what}\n`);
  if (e instanceof StepFailure) {
    process.stderr.write(`  ${e.message}\n`);
    process.stderr.write(`  $ ${e.call.cmd.join(" ")}\n`);
    process.stderr.write(`  exit ${e.call.status}\n`);
    if (e.call.stdout) process.stderr.write(`  stdout: ${truncate(e.call.stdout)}\n`);
    if (e.call.stderr) process.stderr.write(`  stderr: ${truncate(e.call.stderr)}\n`);
  } else {
    process.stderr.write(`  ${e.stack ?? e}\n`);
  }
}

/**
 * Run the nine steps against an already-published, already-checked canary.
 * `ctx`: { target, app ("<org-slug>/platform-canary"), appId, appDir (cwd for
 * `oxyc publish`), oxyc (path to dist/main.mjs), org, project (workspace id),
 * token (a staff bearer — never a publish token), credentialsPath,
 * checkTimeoutSeconds, productionSteps (the CSV production's own CANARY_STEPS
 * resolves to, in canary order), omitSteps (names of `STEPS_A` sandbox A does
 * not run here; default none) }.
 *
 * Exits the process directly on failure, after best-effort cleanup — the same
 * convention `platform-canary-checkpoint.mjs`'s own `fail()` uses, so a
 * caller that imports this (the checkpoint, behind `--sandbox-loop`) gets
 * identical behavior to running this file standalone.
 */
export async function runSandboxLoop(ctx) {
  const allSteps = parseAllSteps(readFileSync(ALL_STEPS_SOURCE, "utf8"));
  const state = {
    stepsA: expectedOrder(sandboxASteps(ctx.omitSteps).join(","), allSteps),
    stepsB: expectedOrder(STEPS_B.join(","), allSteps)
  };
  if ((ctx.omitSteps ?? []).length > 0) {
    process.stdout.write(`   sandbox A omits: ${ctx.omitSteps.join(", ")}\n`);
  }

  await cleanupBoth(ctx, "pre-flight — safe to re-run after an aborted run");

  for (const s of STEPS) {
    try {
      await s.run(ctx, state);
    } catch (e) {
      reportFailure(s.n, s.what, e);
      await cleanupBoth(ctx, "after failure");
      process.exit(1);
    }
    process.stdout.write(`ok ${s.n} ${s.what}\n`);
  }

  // Step 9 recreates dev-loop-a to prove the name frees up; tidy it away
  // rather than leaving a sandbox behind after a green run.
  await cleanupSandbox(ctx, SANDBOX_A);
  process.stdout.write(`\nsandbox loop green: ${STEPS.length} steps through oxyc\n`);
}

// ── standalone entry (mainly for iterating on this script itself) ───────────

const USAGE = `usage: node scripts/ci/platform-canary-sandbox-loop.mjs [options]

  --target <url>           the running server (env CANARY_TARGET)
  --app <org-slug/app>     the canary app (default <org-slug>/platform-canary)
  --org-slug <slug>        the canary's org (default oxy-canary)
  --app-id <uuid>          the canary app's id (required)
  --project <workspace>    the canary's workspace id (required)
  --oxyc <path>            path to oxyc's dist/main.mjs (default sdk/cli/dist/main.mjs)
  --app-dir <path>         the staged canary directory, cwd for \`oxyc publish\` (default .)
  --check-timeout <secs>   per-check timeout passed to oxyc (default 240)
  --omit-steps <csv>       steps sandbox A does not run here (env CANARY_SANDBOX_LOOP_OMIT);
                           CI passes storage_roundtrip

OXY_TOKEN must hold a staff bearer (not a publish token) — the same credential
\`oxyc login\` or a dev-login JWT would give.
`;

/** An environment variable, trimmed; blank counts as absent — mirrors checkpoint.mjs's own. */
function env(name, fallback) {
  return process.env[name]?.trim() || fallback;
}

function parseArgs(argv) {
  const opts = {
    target: env("CANARY_TARGET"),
    orgSlug: "oxy-canary",
    app: undefined,
    appId: undefined,
    project: undefined,
    oxyc: OXYC,
    appDir: process.cwd(),
    checkTimeout: 240,
    omitSteps: parseOmit(env("CANARY_SANDBOX_LOOP_OMIT"))
  };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    const next = () => {
      const v = argv[++i];
      if (v === undefined) {
        process.stderr.write(`${a} needs a value\n${USAGE}`);
        process.exit(2);
      }
      return v;
    };
    if (a === "--target") opts.target = next();
    else if (a === "--app") opts.app = next();
    else if (a === "--org-slug") opts.orgSlug = next();
    else if (a === "--app-id") opts.appId = next();
    else if (a === "--project") opts.project = next();
    else if (a === "--oxyc") opts.oxyc = next();
    else if (a === "--app-dir") opts.appDir = next();
    else if (a === "--check-timeout") opts.checkTimeout = Number(next());
    else if (a === "--omit-steps") opts.omitSteps = parseOmit(next());
    else if (a === "-h" || a === "--help") {
      process.stdout.write(USAGE);
      process.exit(0);
    } else {
      process.stderr.write(`unknown argument ${a}\n${USAGE}`);
      process.exit(2);
    }
  }
  opts.app ??= `${opts.orgSlug}/platform-canary`;
  if (!opts.target || !opts.appId || !opts.project) {
    process.stderr.write(`--target, --app-id and --project are required\n${USAGE}`);
    process.exit(2);
  }
  return opts;
}

async function main() {
  const opts = parseArgs(process.argv.slice(2));
  const token = env("OXY_TOKEN");
  if (!token) {
    process.stderr.write(`OXY_TOKEN must be set to a staff bearer\n${USAGE}`);
    process.exit(2);
  }
  const allSteps = parseAllSteps(readFileSync(ALL_STEPS_SOURCE, "utf8"));
  const productionSteps = deriveCiSteps(allSteps, OMITTED_IN_CI);
  await runSandboxLoop({
    target: opts.target.replace(/\/+$/, ""),
    app: opts.app,
    appId: opts.appId,
    appDir: opts.appDir,
    oxyc: opts.oxyc,
    org: opts.orgSlug,
    project: opts.project,
    token,
    credentialsPath: join(opts.appDir, ".oxy-sandbox-loop-no-credentials.json"),
    checkTimeoutSeconds: opts.checkTimeout,
    productionSteps,
    omitSteps: opts.omitSteps
  });
}

const entry = process.argv[1] ? pathToFileURL(resolve(process.argv[1])).href : "";
if (import.meta.url === entry) await main();
