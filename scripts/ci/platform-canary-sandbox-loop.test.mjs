// The step selection and step table of `platform-canary-sandbox-loop.mjs`, pinned.
//
// No server is involved: these pin the plan §5.1 decisions that can be
// checked without one — the sandbox A/B step names are real canary steps,
// `expectedOrder` reproduces the canary's own run order rather than a second
// hand-written copy of it, and the step table itself is the nine steps the
// plan describes, numbered 1..9 with no gaps or repeats. They also pin which
// credential each step uses: the loop runs as a sandbox agent token, and the
// staff bearer keeps only what `STAFF_ONLY` lists.
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
import {
  mintBody,
  productionRequests,
  SANDBOX_TOKEN_RE,
  STAFF_ONLY,
  scrub
} from "./platform-canary-sandbox-token.mjs";

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

// ── the loop runs as a sandbox agent token ──────────────────────────────────

const loopSource = readFileSync(
  join(REPO, "scripts", "ci", "platform-canary-sandbox-loop.mjs"),
  "utf8"
);

/** The source of `async function step<n>`, up to the brace that closes it. */
function stepSource(n) {
  const m = new RegExp(`\\nasync function step${n}\\(ctx[\\s\\S]*?\\n}\\n`).exec(loopSource);
  assert.ok(m, `no step${n} in the loop`);
  return m[0];
}

test("the mint asks for one app, for one hour, as a sandbox agent token", () => {
  assert.deepEqual(mintBody("app-1"), {
    name: "platform-canary sandbox loop",
    kind: "sandbox_agent",
    apps: ["app-1"],
    expires_in_hours: 1
  });
});

test("a sandbox agent token is its prefix and 36 base62 characters, nothing else", () => {
  const token = `oxy_sbx_${"aB3".repeat(12)}`;
  assert.ok(SANDBOX_TOKEN_RE.test(token));
  for (const not of [
    `oxy_pat_${"aB3".repeat(12)}`,
    `${token}x`,
    token.slice(0, -1),
    `${token}; echo hi`,
    ` ${token}`,
    `oxy_sbx_${"a-3".repeat(12)}`
  ]) {
    assert.equal(SANDBOX_TOKEN_RE.test(not), false, JSON.stringify(not));
  }
});

test("scrub takes both credentials and any token-shaped text out of what is printed", () => {
  const agent = `oxy_sbx_${"aB3".repeat(12)}`;
  const staff = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJzdGFmZiJ9.c2lnbmF0dXJl";
  const stray = `oxy_pat_${"Zz9".repeat(12)}`;
  const printed = scrub(
    `OXY_TOKEN=${agent} failed; Authorization: Bearer ${staff}; also saw ${stray} and ${agent}`,
    [staff, agent]
  );
  for (const secret of [agent, staff, stray]) {
    assert.equal(printed.includes(secret), false, "a credential survived");
  }
  assert.match(printed, /OXY_TOKEN=\[redacted\] failed/);
  assert.match(printed, /oxy_pat_\[redacted\]/);
  // A token nobody listed is still taken out by its shape; ordinary text stays.
  assert.equal(scrub(`saw ${agent}`), "saw oxy_sbx_[redacted]");
  assert.equal(scrub("exit 9: HeldInStaging", [undefined, ""]), "exit 9: HeldInStaging");
  assert.equal(scrub(undefined), "");
});

test("what stays with the staff bearer is listed, with the reason the token may not do it", () => {
  assert.deepEqual(
    STAFF_ONLY.map((s) => s.step),
    [0, 5, 8, 9]
  );
  for (const s of STAFF_ONLY) {
    assert.ok(s.what.length > 0 && s.why.length > 0, `step ${s.step} gives no reason`);
  }
});

test("only the steps STAFF_ONLY lists use the staff bearer; every other call is the token's", () => {
  const listed = new Set(STAFF_ONLY.map((s) => s.step));
  for (const { n } of STEPS) {
    const source = stepSource(n);
    const usesStaff = /\bstaff\(ctx\b/.test(source);
    assert.equal(
      usesStaff,
      listed.has(n),
      `step ${n} ${usesStaff ? "uses" : "does not use"} the staff bearer, and STAFF_ONLY ${
        listed.has(n) ? "lists" : "does not list"
      } it`
    );
    if (usesStaff) assert.match(source, /STAFF_ONLY/, `step ${n} does not say why`);
    // Every step is part of the loop, so every step also runs as the token.
    if (n !== 4) assert.match(source, /\boxyc\(ctx\b|publishSandbox\(ctx|deleteSandbox\(ctx/);
  }
  // Cleanup deletes whatever holds the name, so it is the staff bearer's.
  assert.match(
    loopSource,
    /async function cleanupSandbox[\s\S]*?deleteSandbox\(ctx, name, staff\)/
  );
  // The two helpers are the only way a credential reaches `oxyc`.
  assert.equal((loopSource.match(/OXY_TOKEN: /g) ?? []).length, 1);
});

test("the token sets a sandbox's secret with the named command, never `oxyc api`", () => {
  const source = stepSource(4);
  assert.match(source, /"env",\s*"secret",\s*"set"/);
  assert.equal(/"api"/.test(source), false);
  // No step sends the token through the generic command: `oxyc api` is staff's.
  for (const { n } of STEPS) {
    for (const call of stepSource(n).matchAll(/\b(oxyc|staff)\(ctx, \[\s*"api"/g)) {
      assert.equal(call[1], "staff", `step ${n} sends the token through oxyc api`);
    }
  }
});

test("production is asked directly for four writes, on the app's own paths", () => {
  const asked = productionRequests("app-1", "oxy-canary/platform-canary");
  assert.deepEqual(
    asked.map((r) => r.path),
    [
      "/api/customer-apps/app-1/functions/canary/runs",
      "/api/customer-apps/app-1/publish",
      "/api/customer-apps/app-1/rollback",
      "/customer-apps/oxy-canary/platform-canary/fn/canary"
    ]
  );
  assert.match(stepSource(8), /productionRefusesTheToken\(/);
});

test("the token is revoked on a green run and on a failed one, and then confirmed dead", () => {
  assert.match(loopSource, /function revokeAgentToken[\s\S]*?"tokens", "revoke", "--current"/);
  assert.match(loopSource, /function revokeAgentToken[\s\S]*?assertStatus\(after, 4,/);
  assert.match(
    loopSource,
    /const abort = async[\s\S]*?revokeAfterFailure\(ctx\);\s*process\.exit\(1\)/
  );
  assert.match(loopSource, /revokeAgentToken\(ctx\);/);
});
