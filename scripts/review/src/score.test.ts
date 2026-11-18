import assert from "node:assert/strict";
import { test } from "node:test";
import { candidate, finding, must } from "./fixtures.ts";
import type { GoldenCase, StageDebug } from "./score.ts";
import { scoreCase, summarize } from "./score.ts";

const kase = (expect: GoldenCase["expect"] = [{ file: "a.rs", line: 10, what: "the bug" }]) => ({
  id: "c",
  pr: 1,
  base: "b",
  head: "h",
  expect
});
const bug = candidate({ file: "a.rs", line: 12, title: "the bug" });
const elsewhere = candidate({ file: "z.rs", line: 1 });
const debug = (o: Partial<StageDebug> = {}): StageDebug => ({
  complete: true,
  candidates: [],
  confirmed: [],
  possible: [],
  rejected: [],
  dropped: [],
  known: [],
  ...o
});
const stageOf = (d: StageDebug | undefined) =>
  must(scoreCase(kase(), { findings: [], possible: [], cost: 0, debug: d }).hits[0]);

test("a posted finding near the bug is found and posted", () => {
  const f = finding({ file: "a.rs", line: 11, severity: "should_fix" });
  const hit = must(scoreCase(kase(), { findings: [f], possible: [], cost: 0 }).hits[0]);
  assert.equal(hit.result, "found");
  assert.equal(hit.stage, "posted");
});

test("a candidate the verifier rejected is lost at rejected", () => {
  const hit = stageOf(debug({ candidates: [bug], rejected: [bug] }));
  assert.equal(hit.result, "missed");
  assert.equal(hit.stage, "rejected");
});

test("a confirmed candidate dropped by a filter is lost at filtered", () => {
  const dropped = [{ finding: bug, reason: "below the confidence floor" }];
  assert.equal(stageOf(debug({ candidates: [bug], dropped })).stage, "filtered");
  assert.equal(stageOf(debug({ candidates: [bug], known: dropped })).stage, "filtered");
  assert.equal(stageOf(debug({ candidates: [bug], confirmed: [bug] })).stage, "filtered");
});

test("an uncertain candidate near the bug is possible", () => {
  const hit = must(
    scoreCase(kase(), { findings: [], possible: [bug], cost: 0, debug: debug() }).hits[0]
  );
  assert.equal(hit.result, "possible-only");
  assert.equal(hit.stage, "possible");
  assert.equal(stageOf(debug({ candidates: [bug], possible: [bug] })).stage, "possible");
});

test("no candidate near the bug means no pass saw it", () => {
  const hit = stageOf(debug({ candidates: [elsewhere], rejected: [elsewhere] }));
  assert.equal(hit.stage, "never-found");
  const far = candidate({ file: "a.rs", line: 40 });
  assert.equal(stageOf(debug({ candidates: [far] })).stage, "never-found");
});

test("on a partial review a bug no pass saw is unknown: the pass that would have did not run", () => {
  const stage = (complete: boolean) => stageOf(debug({ candidates: [elsewhere], complete })).stage;
  assert.equal(stage(false), "unknown");
  assert.equal(stage(true), "never-found");
  const seen = debug({ candidates: [bug], rejected: [bug], complete: false });
  assert.equal(stageOf(seen).stage, "rejected", "what was seen is still known");
});

test("a missing or empty result.json is unknown, not a miss", () => {
  assert.equal(stageOf(undefined).stage, "unknown");
  assert.equal(stageOf({ complete: true }).stage, "unknown", "a skipped round writes {}");
  assert.equal(stageOf(undefined).result, "missed");
});

test("the furthest stage reached wins when several candidates are near", () => {
  const other = candidate({ file: "a.rs", line: 9 });
  const hit = stageOf(debug({ candidates: [bug, other], rejected: [bug], possible: [other] }));
  assert.equal(hit.stage, "possible");
});

test("a clean case scores as before", () => {
  const s = scoreCase(kase([]), {
    findings: [finding({ severity: "blocking" })],
    possible: [],
    cost: 1,
    debug: debug()
  });
  assert.deepEqual(s.hits, []);
  assert.equal(s.cleanFalseAlarms, 1);
});

test("summarize reports recall at raw, verified and posted", () => {
  const miss = (d: StageDebug | undefined) =>
    scoreCase(kase(), { findings: [], possible: [], cost: 0, debug: d });
  const posted = scoreCase(kase(), { findings: [finding({ line: 10 })], possible: [], cost: 0 });
  const s = summarize([
    posted,
    miss(debug({ candidates: [bug], possible: [bug] })),
    miss(debug({ candidates: [bug], rejected: [bug] })),
    miss(debug({ candidates: [bug], dropped: [{ finding: bug, reason: "nit cap" }] })),
    miss(debug({ candidates: [elsewhere] })),
    miss(undefined)
  ]);
  assert.equal(s.expected, 6);
  assert.deepEqual(s.stages, {
    posted: 1,
    possible: 1,
    filtered: 1,
    rejected: 1,
    "never-found": 1,
    unknown: 1
  });
  // Five known stages: four were seen, two survived verification, one was posted.
  assert.equal(s.rawRecall, 80);
  assert.equal(s.verifiedRecall, 40);
  assert.equal(s.recall, 17);
});

test("summarize has no stage recall without hits", () => {
  const s = summarize([scoreCase(kase([]), { findings: [], possible: [], cost: 0 })]);
  assert.equal(s.rawRecall, null);
  assert.equal(s.verifiedRecall, null);
});
