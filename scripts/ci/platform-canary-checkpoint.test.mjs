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
import { test } from "node:test";

import {
  ALL_STEPS_SOURCE,
  deriveCiSteps,
  OMITTED_IN_CI,
  parseAllSteps
} from "./platform-canary-checkpoint.mjs";

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
  for (const [name, reason] of Object.entries(OMITTED_IN_CI)) {
    assert.ok(all.includes(name), `OMITTED_IN_CI names ${name}, which is not in ALL_STEPS`);
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
