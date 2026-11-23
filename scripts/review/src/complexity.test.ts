import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { complexityBlock, parseOffenders, scan } from "./complexity.ts";

// Real `bca check` output, copied from a run on this repo: a new offender, a
// worsened one with its percentage, a float value, and the lines around them
// that are not rows.
const OUTPUT = `bca: 9 files not checked (9 generated) — pass --report-skipped to list them
[new] crates/shared/src/probe.rs:2-14: probe: cognitive = 48 (limit 15)
[new] crates/shared/src/probe.rs:2-14: probe: nargs = 6 (limit 5)
[regr +10%] sdk/typescript/src/ui/charts/TrendChart.tsx:134-441: TrendChart: cognitive = 22 (limit 15)
[regr +2%] sdk/typescript/src/ui/charts/TrendChart.tsx:134-441: TrendChart: abc = 111.26994203287786 (limit 40)
bca: filtered 3021 violations via baseline`;

test("parses the rows and ignores everything else", () => {
  const rows = parseOffenders(OUTPUT);
  assert.equal(rows.length, 4);
  assert.deepEqual(rows[0], {
    file: "crates/shared/src/probe.rs",
    lines: "2-14",
    fn: "probe",
    metric: "cognitive",
    value: "48",
    limit: "15",
    kind: "new",
    delta: null
  });
});

test("a worsened entry keeps how much worse it got", () => {
  const worse = parseOffenders(OUTPUT).filter((o) => o.kind === "worse");
  assert.equal(worse.length, 2);
  assert.equal(worse[0]?.delta, "+10%");
});

test("float values are shortened: a prompt does not need 14 decimals", () => {
  const abc = parseOffenders(OUTPUT).find((o) => o.metric === "abc");
  assert.equal(abc?.value, "111.3");
});

// bca tags rows `[new]` / `[regr …]` only when a baseline is in play. With
// none — a tree without `.bca-baseline.toml`, or one it could not read — the
// same rows come out bare. Requiring the tag made this return nothing in
// exactly the case where the most is wrong, which no test caught until a real
// run in a repo without a baseline printed rows this parser threw away.
test("untagged rows still parse, as 'over' rather than new or worse", () => {
  const rows = parseOffenders(
    "./lib.rs:1-9: tangled: cognitive = 42 (limit 15)\n./lib.rs:1-9: tangled: nargs = 6 (limit 5)"
  );
  assert.equal(rows.length, 2);
  assert.equal(rows[0]?.kind, "over");
  assert.equal(rows[0]?.delta, null);
  assert.match(complexityBlock(rows), /`tangled` — cognitive 42 \(limit 15\), over the limit/);
});

test("a qualified name with colons and generics survives", () => {
  const rows = parseOffenders(
    "[new] crates/a/src/b.rs:32-595: Orchestrator<D, S, Ev>::run_pipeline_inner: cognitive = 133 (limit 15)"
  );
  assert.equal(rows.length, 1);
  assert.equal(rows[0]?.fn, "Orchestrator<D, S, Ev>::run_pipeline_inner");
  assert.equal(rows[0]?.metric, "cognitive");
});

test("no offenders renders nothing at all, which is the common case", () => {
  assert.equal(complexityBlock([]), "");
  assert.equal(parseOffenders("bca: filtered 3021 violations via baseline").length, 0);
});

test("the block names each offender and says new or worse", () => {
  const text = complexityBlock(parseOffenders(OUTPUT));
  assert.match(
    text,
    /`crates\/shared\/src\/probe\.rs`:2-14 `probe` — cognitive 48 \(limit 15\), new/
  );
  assert.match(text, /`TrendChart` — cognitive 22 \(limit 15\), worse \+10%/);
  assert.match(text, /<complexity>/);
});

test("the block warns against treating a measurement as a finding", () => {
  const text = complexityBlock(parseOffenders(OUTPUT));
  assert.match(text, /measurements, not findings/);
  assert.match(text, /without making the code easier to read/);
});

test("a long list is capped and says how many were left out", () => {
  const many = Array.from(
    { length: 25 },
    (_, i) => `[new] a/b${i}.rs:1-9: f${i}: cognitive = 20 (limit 15)`
  ).join("\n");
  const text = complexityBlock(parseOffenders(many), 20);
  assert.equal((text.match(/^- `a\//gm) ?? []).length, 20);
  assert.match(text, /…and 5 more\./);
});

test("no bca means no block, not a failed review", async () => {
  assert.deepEqual(await scan({ bin: null, mergeBase: "abc" }), []);
  assert.deepEqual(await scan({ bin: "/nonexistent/bca", mergeBase: "abc" }), []);
});

test("no merge base means no block", async () => {
  assert.deepEqual(await scan({ bin: "/bin/echo", mergeBase: "" }), []);
});

// The exit code is the whole contract: 2-5 are findings, 1 is a tool error.
// A tool error that happens to have printed a row must not reach the prompt —
// a reviewer told about offenders that were never measured is worse than one
// told nothing.
test("exit codes decide: 1 is a tool error even when rows were printed", async () => {
  const dir = mkdtempSync(join(tmpdir(), "bca-stub-"));
  const bin = join(dir, "bca");
  const row = "[new] a.rs:1-2: f: cognitive = 99 (limit 15)";
  try {
    writeFileSync(bin, `#!/bin/sh\necho '${row}'\nexit 1\n`, { mode: 0o755 });
    assert.deepEqual(await scan({ bin, mergeBase: "abc" }), []);

    writeFileSync(bin, `#!/bin/sh\necho '${row}'\nexit 2\n`, { mode: 0o755 });
    const found = await scan({ bin, mergeBase: "abc" });
    assert.equal(found.length, 1);
    assert.equal(found[0]?.fn, "f");

    writeFileSync(bin, `#!/bin/sh\nexit 0\n`, { mode: 0o755 });
    assert.deepEqual(await scan({ bin, mergeBase: "abc" }), []);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
