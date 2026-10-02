// The step selection and step table of `platform-canary-sandbox-loop.mjs`, pinned.
//
// No server is involved: these pin the plan §5.1 decisions that can be
// checked without one — the sandbox A/B step names are real canary steps,
// `expectedOrder` reproduces the canary's own run order rather than a second
// hand-written copy of it, and the step table itself is the nine steps the
// plan describes, numbered 1..9 with no gaps or repeats.
//
//   node --test scripts/ci/platform-canary-sandbox-loop.test.mjs

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import { ALL_STEPS_SOURCE, OMITTED_IN_CI, parseAllSteps } from "./platform-canary-checkpoint.mjs";
import {
  expectedOrder,
  HELD_OP_B,
  HELD_STEP_B,
  INVOCATION_ROW_ID,
  omitHint,
  parseOmit,
  SANDBOX_A,
  SANDBOX_B,
  STEPS,
  STEPS_A,
  STEPS_B,
  sandboxASteps
} from "./platform-canary-sandbox-loop.mjs";

const REPO = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const stepsSource = readFileSync(ALL_STEPS_SOURCE, "utf8");
const allSteps = parseAllSteps(stepsSource);

/** The host ops one step declares, in order, from `STEP_OPS` in the canary's steps.ts. */
function stepOps(step) {
  const m = new RegExp(`\\b${step}:\\s*\\[([^\\]]*)\\]`).exec(stepsSource);
  assert.ok(m, `STEP_OPS has no entry for ${step}`);
  return [...m[1].matchAll(/"([a-zA-Z_.]+)"/g)].map((x) => x[1]);
}

test("dev-loop-a and dev-loop-b are valid dev-<handle> names", () => {
  for (const name of [SANDBOX_A, SANDBOX_B]) {
    assert.match(name, /^dev-[a-z0-9]([a-z0-9-]{0,10}[a-z0-9])?$/, `${name} is not a valid handle`);
  }
  assert.notEqual(SANDBOX_A, SANDBOX_B);
});

test("sandbox A's and B's CANARY_STEPS are real canary steps, and disjoint", () => {
  for (const name of [...STEPS_A, ...STEPS_B]) {
    assert.ok(allSteps.includes(name), `${name} is not in ALL_STEPS (${ALL_STEPS_SOURCE})`);
  }
  assert.deepEqual(
    STEPS_A.filter((s) => STEPS_B.includes(s)),
    [],
    "A and B share a step — they would no longer prove per-sandbox isolation"
  );
});

test("sandbox A's steps are read-only or storage (never warehouse/oltp — plan §5.1 step 4)", () => {
  // Against STEP_OPS in steps.ts and `env_policy::non_production` (see the
  // comment above STEPS_A in platform-canary-sandbox-loop.mjs): sql_read and
  // org_read are production reads; storage_roundtrip works in the sandbox's
  // own silo but uploads through ctx.fetch, which is why it is the step an
  // environment may omit. None of the three writes a shared destination.
  assert.deepEqual([...STEPS_A].sort(), ["org_read", "sql_read", "storage_roundtrip"]);
  for (const step of STEPS_A) {
    const shared = stepOps(step).filter((op) => /^(warehouse|oltp|tx|airhouse)\./.test(op));
    assert.deepEqual(shared, [], `${step} writes a shared destination`);
  }
});

test("sandbox B's step is the one write the canary's manifest leaves unmapped (held)", () => {
  // warehouse_insert writes `canary_warehouse`, and the canary's oxy-app.json
  // declares no `nonProduction.destinations` entry for it — "## Staging"'s
  // "every other write is held" applies, which is what step 6 asserts
  // (HeldInStaging, exit 9).
  assert.deepEqual(STEPS_B, ["warehouse_insert"]);
  assert.equal(HELD_STEP_B, STEPS_B[0]);
  const manifest = JSON.parse(
    readFileSync(join(dirname(dirname(ALL_STEPS_SOURCE)), "oxy-app.json"), "utf8")
  );
  assert.equal(manifest.nonProduction, undefined, "the canary now maps a staging destination");
});

test("the held op step 6 looks for is the FIRST op of sandbox B's step", () => {
  // A held call throws, so the step stops at its first write: the held list
  // names that op and no later one. Looking for `warehouse.insert` — the
  // step's second op — is what the first real run failed on.
  assert.equal(stepOps(HELD_STEP_B)[0], HELD_OP_B);
});

test("an invocation row's id is under the key the listings serialize", () => {
  // `InvocationSummary` is the row of both listings; the loop reads its id to
  // find a call it made. The held route and the run detail call the same id
  // `invocation_id`; a row calls it `id`.
  const dto = readFileSync(
    join(REPO, "crates/app/src/server/api/admin/apps/invocations.rs"),
    "utf8"
  );
  const m = /pub struct InvocationSummary \{([\s\S]*?)\n\}/.exec(dto);
  assert.ok(m, "no InvocationSummary in admin/apps/invocations.rs");
  const fields = [...m[1].matchAll(/^\s+pub (\w+):/gm)].map((x) => x[1]);
  assert.ok(fields.includes(INVOCATION_ROW_ID), `InvocationSummary has no ${INVOCATION_ROW_ID}`);
  assert.ok(!fields.includes("invocation_id"), "the row gained invocation_id: read that instead");
});

test("omitting a step of sandbox A keeps the rest, in order, and refuses a typo", () => {
  assert.deepEqual(sandboxASteps(), STEPS_A);
  assert.deepEqual(sandboxASteps([]), STEPS_A);
  assert.deepEqual(sandboxASteps(["storage_roundtrip"]), ["sql_read", "org_read"]);
  assert.throws(() => sandboxASteps(["storage_roundtip"]), /cannot omit storage_roundtip/);
  assert.throws(() => sandboxASteps(["warehouse_insert"]), /cannot omit warehouse_insert/);
  assert.throws(() => sandboxASteps([...STEPS_A]), /every step of sandbox A is omitted/);
});

test("a check that failed at storage_roundtrip says how to omit it, and nothing else does", () => {
  const failedAt = (step, status = 9) => ({
    status,
    stdout: JSON.stringify({
      checks: [{ name: "canary", passed: false, error: `canary step ${step} failed: HTTP 409` }]
    })
  });
  assert.match(omitHint(failedAt("storage_roundtrip")), /--sandbox-loop-omit storage_roundtrip/);
  assert.equal(omitHint(failedAt("sql_read")), undefined);
  assert.equal(omitHint(failedAt("storage_roundtrip", 0)), undefined);
  assert.equal(omitHint({ status: 9, stdout: "not json" }), undefined);
  assert.equal(omitHint({ status: 9, stdout: JSON.stringify({ checks: [] }) }), undefined);
});

test("--omit-steps parses a comma list, and absent or blank is nothing", () => {
  assert.deepEqual(parseOmit(undefined), []);
  assert.deepEqual(parseOmit(""), []);
  assert.deepEqual(parseOmit(" storage_roundtrip , "), ["storage_roundtrip"]);
});

test("what CI omits from sandbox A is what the checkpoint omits from production", () => {
  // The job passes `--sandbox-loop-omit storage_roundtrip`; the reason is
  // `OMITTED_IN_CI`'s, so the two lists must not drift apart.
  const ci = readFileSync(join(REPO, ".github/workflows/ci.yaml"), "utf8");
  const m = /--sandbox-loop-omit ([a-z_,]+)/.exec(ci);
  assert.ok(m, "ci.yaml no longer passes --sandbox-loop-omit");
  const omitted = parseOmit(m[1]);
  assert.deepEqual(
    omitted,
    STEPS_A.filter((step) => step in OMITTED_IN_CI),
    "ci.yaml omits a different set than OMITTED_IN_CI ∩ sandbox A's steps"
  );
  assert.doesNotThrow(() => sandboxASteps(omitted));
});

test("expectedOrder reproduces the canary's own run order, not CANARY_STEPS' CSV order", () => {
  assert.deepEqual(expectedOrder("org_read,sql_read", allSteps), ["sql_read", "org_read"]);
  assert.deepEqual(expectedOrder(STEPS_A.join(","), allSteps), STEPS_A);
  assert.deepEqual(expectedOrder("", allSteps), []);
  assert.deepEqual(expectedOrder("not_a_real_step", allSteps), []);
});

test("the step table is exactly nine steps, numbered 1..9 with no gaps", () => {
  assert.equal(STEPS.length, 9);
  assert.deepEqual(
    STEPS.map((s) => s.n),
    [1, 2, 3, 4, 5, 6, 7, 8, 9]
  );
  for (const s of STEPS) {
    assert.equal(typeof s.what, "string");
    assert.ok(s.what.length > 0, `step ${s.n} has no description`);
    assert.equal(typeof s.run, "function");
  }
});
