import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { test } from "node:test";
import * as config from "../config.ts";
import { parseUnifiedDiff } from "./diff.ts";
import { candidate, finding, must, prMeta, round } from "./fixtures.ts";
import { lensesFinished } from "./load.ts";
import { buildPlan, guidanceFor, selectLenses } from "./plan.ts";
import {
  contextBlock,
  fill,
  generalPrompt,
  lensPrompt,
  parseLens,
  recheckPrompt,
  template,
  verifyPrompt
} from "./prompts.ts";
import type { DiffFile, PassSummary } from "./types.ts";

const passSummary = (ok: boolean): PassSummary => ({
  ok,
  subtype: ok ? "success" : "crashed",
  cost: 0,
  ms: 0,
  turns: 0,
  tools: {}
});

function diffOf(path: string, added: string[]): DiffFile {
  const [file] = parseUnifiedDiff(`diff --git a/${path} b/${path}
--- a/${path}
+++ b/${path}
@@ -1,1 +1,${added.length + 1} @@
 context
${added.map((l) => `+${l}`).join("\n")}
`);
  return must(file, "diff file");
}

test("lockfiles and binaries are ignored; workflows and manifests are reviewed", () => {
  const files = [
    diffOf("Cargo.lock", ["x"]),
    diffOf(".github/workflows/ci.yaml", ["on: push"]),
    diffOf("crates/app/Cargo.toml", ["serde = 1"])
  ];
  const plan = buildPlan({ files, config, exists: () => false });
  assert.deepEqual(plan.ignored, ["Cargo.lock"]);
  assert.deepEqual(
    plan.reviewable.map((f) => f.path),
    [".github/workflows/ci.yaml", "crates/app/Cargo.toml"]
  );
  assert.deepEqual(
    plan.lenses.map((l) => l.id),
    ["ci", "architecture"]
  );
});

test("a lens fires on an added pattern only inside its patchPaths", () => {
  const server = diffOf("crates/server/src/drivers/x.rs", ["tokio::spawn(async move {});"]);
  const cli = diffOf("crates/core/src/cli.rs", ["tokio::spawn(async move {});"]);
  assert.deepEqual(
    selectLenses([server], config.LENSES, 5).selected.map((l) => l.id),
    ["task-spec"]
  );
  assert.deepEqual(selectLenses([cli], config.LENSES, 5).selected, []);
});

test("lenses are capped in priority order and the rest are named", () => {
  const files = [
    diffOf("crates/authz/src/lib.rs", ["x"]),
    diffOf("crates/migration/src/m1.rs", ["ALTER TABLE t DROP COLUMN c"]),
    diffOf(".github/workflows/a.yaml", ["x"]),
    diffOf("crates/x/Cargo.toml", ["x"])
  ];
  const { selected, dropped } = selectLenses(files, config.LENSES, 3);
  assert.deepEqual(
    selected.map((l) => l.id),
    ["authz", "data-placement", "migrations"]
  );
  assert.deepEqual(
    dropped.map((l) => l.id),
    ["ci", "architecture"]
  );
});

test("incremental plans review only files changed since the last head", () => {
  const files = [diffOf("a.rs", ["1"]), diffOf("b.rs", ["2"])];
  const plan = buildPlan({ files, config, exists: () => false, changedSince: new Set(["b.rs"]) });
  assert.equal(plan.mode, "incremental");
  assert.deepEqual(plan.inScope, ["b.rs"]);
});

test("a re-push runs a lens on what it changed, not on lines an earlier round's lens read", () => {
  const path = "crates/app/src/agentic_wiring/project_ctx/mod.rs";
  // The PR added `password` on its first push; this push adds an unrelated line.
  const whole = [diffOf(path, ["let password = mint();", "let x = 1;"])];
  const delta = [diffOf(path, ["let x = 1;"])];
  const plan = (covered: Set<string>, d = delta) =>
    buildPlan({
      files: whole,
      config,
      exists: () => false,
      changedSince: new Set([path]),
      delta: d,
      covered
    });
  assert.deepEqual(plan(new Set(["security"])).lenses, []);
  // The push adds a trigger of its own: the lens runs on it.
  const fresh = plan(new Set(["security"]), [diffOf(path, ["let password = again();"])]);
  assert.deepEqual(
    fresh.lenses.map((l) => l.id),
    ["security"]
  );
  assert.match(must(fresh.lenses[0]).reason, /^adds `password`/);
  // A lens that never finished on this PR still reads the earlier lines.
  const unread = plan(new Set());
  assert.deepEqual(
    unread.lenses.map((l) => l.id),
    ["security"]
  );
  assert.match(must(unread.lenses[0]).reason, /no full review of this PR has run this lens/);
});

test("a full review, or an incremental one without a delta, matches the whole PR diff", () => {
  const path = "crates/app/src/agentic_wiring/project_ctx/mod.rs";
  const whole = [diffOf(path, ["let password = mint();"])];
  const ids = (p: ReturnType<typeof buildPlan>) => p.lenses.map((l) => l.id);
  assert.deepEqual(
    ids(buildPlan({ files: whole, config, exists: () => false, covered: new Set(["security"]) })),
    ["security"]
  );
  assert.deepEqual(
    ids(
      buildPlan({
        files: whole,
        config,
        exists: () => false,
        changedSince: new Set([path]),
        covered: new Set(["security"])
      })
    ),
    ["security"]
  );
});

test("guidance is the nearest CLAUDE.md above each file", () => {
  const exists = (p: string) =>
    ["crates/app/CLAUDE.md", "crates/agentic/CLAUDE.md", "crates/agentic/core/CLAUDE.md"].includes(
      p
    );
  assert.deepEqual(
    guidanceFor(["crates/app/src/x.rs", "crates/agentic/core/src/a.rs", "README.md"], exists),
    ["crates/agentic/core/CLAUDE.md", "crates/app/CLAUDE.md"]
  );
});

test("every lens has a prompt with a Focus line, and every referenced repo file exists", () => {
  const root = new URL("../../../", import.meta.url);
  for (const lens of config.LENSES) {
    const { focus, body } = parseLens(template(lens.prompt));
    assert.ok(focus.length > 10, `${lens.id} focus`);
    assert.ok(body.includes("Check:"), `${lens.id} checklist`);
    assert.ok(lens.paths?.length || lens.patch, `${lens.id} trigger`);
    for (const ref of body.matchAll(/`((?:\.claude|internal-docs|crates)\/[^`*]+\.(?:md|rs))`/g)) {
      assert.ok(
        existsSync(new URL(ref[1] ?? "", root)),
        `${lens.id} names ${ref[1]}, which does not exist`
      );
    }
  }
});

test("the reviewer's tools are read-only", () => {
  for (const t of config.ALLOWED_TOOLS)
    assert.ok(
      !/^(Edit|Write|NotebookEdit|Bash\((gh pr (comment|review|merge)|git (push|commit|checkout|reset)))/.test(
        t
      ),
      t
    );
});

test("fill refuses a missing variable and leaves workflow expressions alone", () => {
  assert.throws(() => fill("{{a}} {{b}}", { a: 1 }), /\{\{b\}\}/);
  assert.equal(
    fill("run: ${{ github.event.pr }} {{x}}", { x: "ok" }),
    "run: ${{ github.event.pr }} ok"
  );
});

test("every prompt assembles, and user text is not re-templated", () => {
  const files = [diffOf("crates/authz/src/lib.rs", ["enforce_guard(x)"])];
  const plan = buildPlan({ files, config, exists: () => false });
  const meta = prMeta({
    number: 7,
    title: "feat: {{rules}}",
    body: "<!-- hidden -->Hello {{context}}"
  });
  const prior = [finding({ id: "F1", severity: "nit", file: "a.rs", line: 3, title: "old" })];
  const context = contextBlock({
    repo: "o/r",
    meta,
    plan,
    mergeBase: "aaa",
    head: "bbb",
    prior,
    since: null,
    bodyChars: 100
  });
  assert.ok(context.includes("Hello {{context}}") && !context.includes("hidden"));
  assert.ok(context.includes("F1 (nit)"));
  assert.ok(generalPrompt({ context, lenses: plan.lenses, maxFindings: 5 }).includes("authz"));
  assert.ok(
    lensPrompt({ lens: must(plan.lenses[0]), context, maxFindings: 5 }).includes("enforce_guard")
  );
  assert.ok(
    verifyPrompt({
      candidates: [
        candidate({ cid: "C1", title: "t", file: "a", severity: "nit", reasoning: "r", fix: "f" })
      ],
      context,
      mergeBase: "aaa",
      head: "bbb",
      maxFindings: 5
    }).includes('"C1"')
  );
  assert.ok(
    recheckPrompt({
      findings: prior,
      replies: new Map([["F1", ["dev: covered by caller"]]]),
      context,
      since: "aaa",
      mergeBase: "base",
      head: "bbb"
    }).includes("covered by caller")
  );
});

test("a pattern in prose or in the reviewer's own config does not trigger a lens", () => {
  const doc = diffOf("internal-docs/x.md", [
    "Never call `Command::new` with user data or `ctx.oltp`."
  ]);
  const own = diffOf("scripts/review/config.ts", ["patch: /enforce_guard|role_guards/"]);
  assert.deepEqual(
    selectLenses([doc, own], config.LENSES, 5, config.PATCH_EXCLUDE).selected.map((l) => l.id),
    ["ci"]
  );
});

test("a lens counts as having read the PR only once it finished on a full review", () => {
  const prev = {
    rounds: [
      round({ passes: { security: passSummary(true), general: passSummary(true) } }),
      round({ passes: { authz: passSummary(false) } }),
      // An incremental round read only the files it changed (F1 on #4168): a lens that
      // finished there has not read the rest of the PR.
      round({ mode: "incremental", passes: { routes: passSummary(true) } }),
      round()
    ]
  };
  assert.deepEqual([...lensesFinished(prev)], ["security"]);
});
