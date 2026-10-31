// The step selection of `platform-canary-checkpoint.mjs`, pinned.
//
// The CI step list is DERIVED: every name the canary exports in `ALL_STEPS`
// (functions/steps.ts) minus `OMITTED_IN_CI`, whose entries carry their
// reasons. So a step added to the canary runs in CI the day it lands, and the
// only way to keep one out is to name it here with a reason. These tests make
// the two lists a partition of the real one: an omitted name that is not a
// real step fails (a typo, or a step that was renamed or removed), and a step
// in neither list is impossible by construction, which the partition test
// states rather than assumes.
//
//   node --test scripts/ci/platform-canary-checkpoint.test.mjs

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import {
  ALL_STEPS_SOURCE,
  deriveCiSteps,
  OMITTED_IN_CI,
  OMITTED_WITHOUT_AIRHOUSE,
  omissions,
  parseAllSteps,
  withDatabase
} from "./platform-canary-checkpoint.mjs";

const REPO = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const source = readFileSync(ALL_STEPS_SOURCE, "utf8");
const all = parseAllSteps(source);

test("the canary's ALL_STEPS parses into a non-empty list of unique step names", () => {
  assert.ok(all.length >= 8, `only ${all.length} steps parsed from ${ALL_STEPS_SOURCE}`);
  assert.deepEqual([...new Set(all)], all, "duplicate step names");
  for (const name of all) assert.match(name, /^[a-z][a-z0-9_]*$/, `odd step name ${name}`);
  for (const known of ["warehouse_insert", "shape_zoo", "secrets_roundtrip", "check_in"]) {
    assert.ok(all.includes(known), `${known} is missing from ALL_STEPS`);
  }
});

test("every omitted name is a real step, and every omission has a reason", () => {
  for (const [name, reason] of Object.entries(omissions(false))) {
    assert.ok(all.includes(name), `an omission names ${name}, which is not in ALL_STEPS`);
    assert.ok(typeof reason === "string" && reason.length > 20, `${name} has no reason`);
  }
});

test("CI steps and omissions partition ALL_STEPS, in the canary's order", () => {
  const ci = deriveCiSteps(all, OMITTED_IN_CI);
  const omitted = Object.keys(OMITTED_IN_CI);
  // Nothing in both, nothing in neither.
  assert.deepEqual(
    ci.filter((s) => omitted.includes(s)),
    [],
    "a step is both run and omitted"
  );
  assert.deepEqual(
    all.filter((s) => !ci.includes(s) && !omitted.includes(s)),
    [],
    "a step is in neither list"
  );
  assert.deepEqual(
    all.filter((s) => !omitted.includes(s)),
    ci,
    "CI steps are not ALL_STEPS minus the omissions, in order"
  );
  assert.equal(ci.length + omitted.length, all.length);
});

test("the parser refuses a source it cannot read exactly", () => {
  assert.throws(() => parseAllSteps("export const OTHER = [];"), /ALL_STEPS/);
  assert.throws(
    () => parseAllSteps('export const ALL_STEPS: StepName[] = ["a", b];'),
    /ALL_STEPS/,
    "an unquoted entry must not be silently dropped"
  );
  assert.throws(() => parseAllSteps("export const ALL_STEPS: StepName[] = [];"), /ALL_STEPS/);
  assert.deepEqual(
    parseAllSteps('export const ALL_STEPS: StepName[] = [\n  "one", // first\n  "two_b"\n];\n'),
    ["one", "two_b"]
  );
});

test("a step absent from the source cannot be omitted", () => {
  assert.throws(() => deriveCiSteps(["one", "two"], { three: "gone" }), /three/);
});

test("--airhouse runs airhouse_reads, and only it comes back", () => {
  assert.deepEqual(Object.keys(OMITTED_WITHOUT_AIRHOUSE), ["airhouse_reads"]);
  assert.ok(!("airhouse_reads" in OMITTED_IN_CI), "CI boots Airhouse, so it runs airhouse_reads");
  assert.equal(omissions(true), OMITTED_IN_CI);
  assert.deepEqual(Object.keys(omissions(false)), [
    ...Object.keys(OMITTED_IN_CI),
    "airhouse_reads"
  ]);
  assert.ok(deriveCiSteps(all, omissions(true)).includes("airhouse_reads"));
  assert.ok(!deriveCiSteps(all, omissions(false)).includes("airhouse_reads"));
});

test("CI's checkpoint run passes --airhouse, so OMITTED_IN_CI is what it omits", () => {
  // The job boots docker-compose.airhouse.yml for this flag. Drop the flag and
  // airhouse_reads silently leaves CI; this is what notices.
  const ci = readFileSync(join(REPO, ".github/workflows/ci.yaml"), "utf8");
  const run = /node scripts\/ci\/platform-canary-checkpoint\.mjs((?:[^\n]*\\\n)*[^\n]*)/.exec(ci);
  assert.ok(run, "ci.yaml no longer runs platform-canary-checkpoint.mjs");
  assert.match(run[1], /\s--airhouse(\s|$)/, "ci.yaml runs the checkpoint without --airhouse");
});

test("withDatabase appends to the databases list, so the default database stays first", () => {
  const flush = "databases:\n- name: canary_warehouse\n  type: clickhouse\nmodels: []\n";
  assert.equal(
    withDatabase(flush, "canary_airhouse", "airhouse_managed"),
    "databases:\n- name: canary_warehouse\n  type: clickhouse\n- name: canary_airhouse\n  type: airhouse_managed\nmodels: []\n"
  );
  const indented = "databases:\n  - name: canary_warehouse\n    type: clickhouse\n\n";
  assert.equal(
    withDatabase(indented, "canary_airhouse", "airhouse_managed"),
    "databases:\n  - name: canary_warehouse\n    type: clickhouse\n  - name: canary_airhouse\n    type: airhouse_managed\n"
  );
  assert.equal(
    withDatabase("models: []\n", "canary_airhouse", "airhouse_managed"),
    "models: []\ndatabases:\n  - name: canary_airhouse\n    type: airhouse_managed\n"
  );
});
