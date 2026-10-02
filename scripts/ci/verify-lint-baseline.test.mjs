import assert from "node:assert/strict";
import { test } from "node:test";
import { grown, totalsByRule } from "./verify-lint-baseline.mjs";

const RULE = "typescript/no-floating-promises";
const OTHER = "typescript/no-base-to-string";

test("totals are summed per rule across files", () => {
  const totals = totalsByRule({
    "a.ts": { [RULE]: { count: 2 }, [OTHER]: { count: 1 } },
    "b.ts": { [RULE]: { count: 3 } }
  });
  assert.equal(totals.get(RULE), 5);
  assert.equal(totals.get(OTHER), 1);
});

test("one more violation of a rule is growth", () => {
  const base = { "a.ts": { [RULE]: { count: 2 } } };
  const head = { "a.ts": { [RULE]: { count: 3 } } };
  assert.deepEqual(grown(base, head), [{ rule: RULE, before: 2, after: 3 }]);
});

test("a rule the base never had is growth from zero", () => {
  const base = { "a.ts": { [RULE]: { count: 2 } } };
  const head = { "a.ts": { [RULE]: { count: 2 }, [OTHER]: { count: 1 } } };
  assert.deepEqual(grown(base, head), [{ rule: OTHER, before: 0, after: 1 }]);
});

test("moving a file's entries to another path is not growth", () => {
  const base = { "old/a.ts": { [RULE]: { count: 2 } } };
  const head = { "new/a.ts": { [RULE]: { count: 1 } }, "new/b.ts": { [RULE]: { count: 1 } } };
  assert.deepEqual(grown(base, head), []);
});

test("a shrinking baseline is not growth", () => {
  const base = { "a.ts": { [RULE]: { count: 2 } }, "b.ts": { [OTHER]: { count: 1 } } };
  const head = { "a.ts": { [RULE]: { count: 1 } } };
  assert.deepEqual(grown(base, head), []);
});
