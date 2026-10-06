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
 * the publish cwd), its staff `dev-login` token, and the workspace build of
 * `oxyc`. Can also run standalone against an already-published canary — see
 * `main()` below — which is mainly useful for iterating on this script itself.
 *
 * **The loop runs as a sandbox agent token** (`oxy_sbx_…`,
 * `internal-docs/2026-10-03-sandbox-agent-credential-design.md` §7.2): the
 * credential an agent holds. The staff bearer mints one for the canary app,
 * for an hour, the way a person does in a browser (`POST /api/user/tokens`,
 * which takes a session and no token). Every step of the loop then uses that
 * token, and it is revoked at the end, on a green run and on a failed one.
 * The staff bearer keeps only what the token is not allowed to do —
 * `STAFF_ONLY` names each and why.
 *
 * `STAFF_ONLY`, the mint and what keeps both credentials out of the output
 * are in `platform-canary-sandbox-token.mjs`.
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
import {
  mintAgentToken,
  productionRefusesTheToken,
  scrub
} from "./platform-canary-sandbox-token.mjs";

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
 * nothing held. Nor is `storage_roundtrip`'s upload: the non-production table
 * holds a mutating `ctx.fetch` (`Fetch => Hold`), except a PUT to a URL the
 * same invocation's `ctx.storage.getUploadUrl` minted into the sandbox's own
 * silo (`EnvPolicy::decide_on_fetch`), which is what the step sends.
 * `storage_roundtrip` is in the default list by decision, and is the step to
 * omit (`--omit-steps`, below) where it cannot run: `ctx.fetch` sends only
 * HTTPS to a public host, in every environment, so the PUT is refused where
 * the object store is on loopback — a dev box's or the CI runner's MinIO (the
 * reason the checkpoint's own `OMITTED_IN_CI` gives).
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

/** Shell `node <oxyc> <args>` with `bearer` as `OXY_TOKEN`; stdout and stderr captured, never inherited. */
function run(ctx, bearer, args, { cwd } = {}) {
  const cmd = ["node", ctx.oxyc, ...args];
  const r = spawnSync(cmd[0], cmd.slice(1), {
    cwd: cwd ?? ctx.appDir,
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
    env: { ...process.env, OXY_TOKEN: bearer, OXY_CREDENTIALS_PATH: ctx.credentialsPath }
  });
  return { cmd, status: r.status ?? -1, stdout: r.stdout ?? "", stderr: r.stderr ?? "" };
}

/** `oxyc` as the sandbox agent token: every step of the loop. */
function oxyc(ctx, args, opts) {
  return run(ctx, ctx.agentToken, args, opts);
}

/** `oxyc` as the staff bearer: only what `STAFF_ONLY` lists. */
function staff(ctx, args, opts) {
  return run(ctx, ctx.token, args, opts);
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
    "ctx.fetch, which sends only HTTPS to a public host, so it is refused where the object " +
    "store is on loopback. Against such a store, re-run with --sandbox-loop-omit " +
    "storage_roundtrip (or CANARY_SANDBOX_LOOP_OMIT=storage_roundtrip)"
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

/** `oxyc env delete … --wait`, by `as`: the token (`oxyc`) in step 9, `staff` for cleanup. */
function deleteSandbox(ctx, name, as = oxyc) {
  return as(ctx, [
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

/**
 * Cleanup is the staff bearer's: it deletes whatever holds the name, whoever
 * created it. Exit 0 (deleted) or 5 (never existed) are both fine; anything
 * else is a loud warning, not fatal.
 */
async function cleanupSandbox(ctx, name) {
  const call = deleteSandbox(ctx, name, staff);
  if (call.status === 0 || call.status === 5) return;
  process.stderr.write(
    `  cleanup: deleting ${name} exited ${call.status} (continuing)\n` +
      `    stderr: ${scrub(call.stderr.slice(0, 500), [ctx.token, ctx.agentToken])}\n`
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
  for (const { name, steps } of [
    { name: SANDBOX_A, steps: state.stepsA },
    { name: SANDBOX_B, steps: STEPS_B }
  ]) {
    // The named command, not `oxyc api`: a sandbox agent token is refused the
    // generic one before any request, and this is the route it sets a
    // sandbox's secret through.
    const call = oxyc(ctx, [
      "env",
      "secret",
      "set",
      ctx.app,
      "CANARY_STEPS",
      "--app-env",
      name,
      "--value",
      steps.join(","),
      "--target",
      ctx.target,
      "--json"
    ]);
    assertStatus(call, 0, `set CANARY_STEPS for ${name}`);
    const change = parseJsonOrThrow(call, `set CANARY_STEPS for ${name}`);
    assertJson(
      call,
      change.key === "CANARY_STEPS" && change.environment === name,
      `set CANARY_STEPS for ${name}: answered ${JSON.stringify(change)}`
    );
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

  // STAFF_ONLY: the answer is read on the admin surface. The token itself
  // read this run's detail while `checks run` polled it, on its own mount.
  const detail = staff(ctx, [
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
  // The token is refused production before any request (exit 2) by `oxyc`,
  // and by the server when asked directly.
  const refused = oxyc(ctx, ["checks", "run", ctx.app, "--target", ctx.target, "--json"]);
  assertStatus(refused, 2, "checks run on production as the sandbox agent token");
  await productionRefusesTheToken(ctx.target, ctx.agentToken, ctx.appId, ctx.app);

  // STAFF_ONLY: production is not the token's.
  const call = staff(ctx, [
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

  const detail = staff(ctx, [
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

  const history = ["invocations", "list", ctx.app, "--app-env", SANDBOX_A];
  const flags = ["--target", ctx.target, "--json"];

  // The token has no dev-loop-a now, so the name's rows are not its to read.
  const gone = oxyc(ctx, [...history, ...flags]);
  assertStatus(gone, 5, "invocations list for dev-loop-a as the token, after delete");

  // STAFF_ONLY: the history is kept, and staff read it by the name.
  const listA = staff(ctx, [...history, ...flags]);
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

  // The new dev-loop-a is another sandbox with the same name: the token reads
  // none of what the deleted one ran, though the rows are still there.
  const fresh = oxyc(ctx, [...history, ...flags]);
  assertStatus(fresh, 0, "invocations list for the recreated dev-loop-a");
  const carried = parseJsonOrThrow(fresh, "invocations list for the recreated dev-loop-a");
  assertJson(
    fresh,
    (carried.invocations ?? []).length === 0,
    `the recreated dev-loop-a shows the deleted one's rows: ${JSON.stringify(carried.invocations)}`
  );
}

/**
 * End the token, and confirm it ended: `oxyc tokens revoke --current`, then
 * one more request with it, which must be refused as an auth failure (exit
 * 4) — not as a sandbox that happens to be missing.
 */
function revokeAgentToken(ctx) {
  const revoke = oxyc(ctx, ["tokens", "revoke", "--current", "--target", ctx.target]);
  assertStatus(revoke, 0, "tokens revoke --current");
  const after = oxyc(ctx, ["env", "list", ctx.app, "--target", ctx.target, "--json"]);
  assertStatus(after, 4, "env list with the revoked token");
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
  {
    n: 8,
    what: "production refuses the token; its own checks run (staff) is still green",
    run: step8
  },
  {
    n: 9,
    what: "delete both sandboxes; history kept; the name frees up, and shows the token none of it",
    run: step9
  }
];

// ── driver ───────────────────────────────────────────────────────────────────

function truncate(s) {
  return s.length > 4000 ? `${s.slice(0, 4000)}… (truncated)` : s;
}

/** `secrets`: both credentials, taken out of everything printed (`scrub`). */
function reportFailure(n, what, e, secrets) {
  const clean = (text) => scrub(text, secrets);
  process.stderr.write(`\nFAIL step ${n}: ${what}\n`);
  if (e instanceof StepFailure) {
    process.stderr.write(`  ${clean(e.message)}\n`);
    process.stderr.write(`  $ ${clean(e.call.cmd.join(" "))}\n`);
    process.stderr.write(`  exit ${e.call.status}\n`);
    if (e.call.stdout) process.stderr.write(`  stdout: ${clean(truncate(e.call.stdout))}\n`);
    if (e.call.stderr) process.stderr.write(`  stderr: ${clean(truncate(e.call.stderr))}\n`);
  } else {
    process.stderr.write(`  ${clean(e.stack ?? e)}\n`);
  }
}

/**
 * End the token after a failed run, whatever state the run left it in: an
 * exit that is not 0 is reported and does not mask the failure being
 * handled. The token also ends by itself within the hour it was minted for.
 */
function revokeAfterFailure(ctx) {
  if (!ctx.agentToken) return;
  const revoke = oxyc(ctx, ["tokens", "revoke", "--current", "--target", ctx.target]);
  if (revoke.status !== 0) {
    process.stderr.write(`  the sandbox agent token was not revoked (exit ${revoke.status})\n`);
  }
}

/**
 * Run the nine steps against an already-published, already-checked canary.
 * `ctx`: { target, app ("<org-slug>/platform-canary"), appId, appDir (cwd for
 * `oxyc publish`), oxyc (path to dist/main.mjs), org, project (workspace id),
 * token (a staff **session** bearer, e.g. a dev-login JWT — the mint of the
 * sandbox agent token takes a session and refuses every API token),
 * credentialsPath,
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

  // A copy: the caller's `ctx` keeps holding the staff bearer alone.
  ctx = { ...ctx };
  const secrets = () => [ctx.token, ctx.agentToken];
  /** Report `e`, delete both sandboxes, end the token, and exit non-zero. */
  const abort = async (n, what, e) => {
    reportFailure(n, what, e, secrets());
    await cleanupBoth(ctx, "after failure");
    revokeAfterFailure(ctx);
    process.exit(1);
  };

  try {
    ctx.agentToken = await mintAgentToken(ctx.target, ctx.token, ctx.appId);
  } catch (e) {
    await abort(0, "mint a sandbox agent token for the canary app with the staff bearer", e);
  }

  for (const s of STEPS) {
    try {
      await s.run(ctx, state);
    } catch (e) {
      await abort(s.n, s.what, e);
    }
    process.stdout.write(`ok ${s.n} ${s.what}\n`);
  }

  // Step 9 recreates dev-loop-a to prove the name frees up; tidy it away
  // rather than leaving a sandbox behind after a green run.
  await cleanupSandbox(ctx, SANDBOX_A);
  try {
    revokeAgentToken(ctx);
  } catch (e) {
    await abort(STEPS.length + 1, "revoke the sandbox agent token; it is refused from then on", e);
  }
  process.stdout.write("ok revoked the sandbox agent token; its next request was refused\n");
  process.stdout.write(
    `\nsandbox loop green: ${STEPS.length} steps through oxyc as a sandbox agent token\n`
  );
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

OXY_TOKEN must hold a staff session bearer — a dev-login JWT. The loop mints
its own sandbox agent token from it and runs as that; the mint takes a session
and refuses every API token, an \`oxyc login\` one included.
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
    process.stderr.write(`OXY_TOKEN must be set to a staff session bearer\n${USAGE}`);
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
