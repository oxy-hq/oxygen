import assert from "node:assert/strict";
import { test } from "node:test";
import * as config from "../config.ts";
import { parseUnifiedDiff } from "./diff.ts";
import { must, prMeta, reviewState, round } from "./fixtures.ts";
import { buildPlan, selectChecklist } from "./plan.ts";
import { checklistBlock, contextBlock, generalPrompt } from "./prompts.ts";
import { convergenceNote } from "./render.ts";
import type { DiffFile } from "./types.ts";

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

const picked = (files: DiffFile[]): string[] =>
  selectChecklist(files, config.CHECKLIST, 99, config.PATCH_EXCLUDE).selected.map((c) => c.id);

test("every checklist item is traceable, triggerable and short", () => {
  const ids = config.CHECKLIST.map((c) => c.id);
  assert.equal(new Set(ids).size, ids.length, "ids are unique");
  for (const item of config.CHECKLIST) {
    assert.match(item.id, /^[a-z0-9]+(-[a-z0-9]+)*$/, `${item.id}: kebab-case id`);
    assert.match(item.source, /#\d+|\.md\b/, `${item.id}: source names a PR or a doc`);
    assert.ok(item.paths?.length || item.patch, `${item.id}: has a trigger`);
    assert.ok(item.check.length <= 450, `${item.id}: check is at most 450 characters`);
  }
});

// Each item must fire on the shape of the incident it came from; an item whose trigger
// cannot see its own incident is dead weight.
const INCIDENTS: [string, string, string][] = [
  ["persisted-shape-compat", "crates/app/src/reconcile/config.rs", "#[serde(default)]"],
  [
    "clickhouse-statement-tail",
    "crates/app/src/server/api/custom_apps_functions/data_audit.rs",
    "let sql = sqlcommenter::append(stmt, &tags);"
  ],
  ["ducklake-ddl", "crates/airhouse/src/ddl.rs", '"CREATE TABLE t (id INT PRIMARY KEY)"'],
  ["stamp-before-fatal-work", "crates/app/src/server/health/smoke.rs", "row.last_smoke_at = now;"],
  ["duckdb-open", "crates/core/src/db.rs", "let conn = duckdb::Connection::open(path)?;"],
  [
    "per-request-scan",
    "crates/app/src/server/auth/lookup.rs",
    '.filter(Expr::cust("LOWER(email)").eq(email))'
  ],
  [
    "oauth-refresh-rotation",
    "crates/integration/quickbooks/src/token.rs",
    "let t = refresh_token;"
  ],
  ["abort-leaves-loading", "sdk/react/src/useQuery.ts", 'if (e.name === "AbortError") return;'],
  ["clock-in-tests", "crates/app/tests/platform/usage.rs", "let anchor = Utc::now();"],
  ["review-state-contract", "scripts/review/src/state.ts", "export const x = 1;"]
];

test("each checklist item fires on the shape of its own incident", () => {
  assert.deepEqual(
    INCIDENTS.map(([id]) => id).sort(),
    config.CHECKLIST.map((c) => c.id).sort(),
    "every item has an incident example here"
  );
  for (const [id, path, line] of INCIDENTS)
    assert.ok(picked([diffOf(path, [line])]).includes(id), `${id} fires on ${path}`);
});

const QUIET: [path: string, line: string][] = [
  ["docs/clickhouse.md", "INSERT INTO t VALUES (1)"],
  ["crates/core/src/pg.rs", "CREATE UNIQUE INDEX x"],
  ["crates/core/src/usage.rs", "let now = Utc::now();"],
  ["crates/core/src/pr.rs", "// the Roadmap trailer"]
];

test("checklist items stay quiet outside their paths and in prose", () => {
  for (const [path, line] of QUIET)
    assert.deepEqual(picked([diffOf(path, [line])]), [], `${path}: ${line}`);
});

test("an unfiltered Sea-ORM read in a handler is a per-request scan; a filtered one is not", () => {
  const handler = "crates/app/src/server/api/users.rs";
  assert.ok(
    picked([diffOf(handler, ["let all = User::find().all(db).await?;"])]).includes(
      "per-request-scan"
    )
  );
  assert.ok(
    !picked([diffOf(handler, ["let one = User::find().filter(c).all(db).await?;"])]).includes(
      "per-request-scan"
    )
  );
});

test("triggered checklist items past the cap are named, in checklist order", () => {
  const files = [
    diffOf("crates/core/src/db.rs", ["#[serde(default)]", "let c = duckdb::Connection::open(p);"])
  ];
  const { selected, dropped } = selectChecklist(files, config.CHECKLIST, 1, config.PATCH_EXCLUDE);
  assert.deepEqual(
    selected.map((c) => c.id),
    ["persisted-shape-compat"]
  );
  assert.deepEqual(dropped, ["duckdb-open"]);
});

test("a re-push asks the checklist only about what it changed", () => {
  const path = "crates/core/src/db.rs";
  const whole = [diffOf(path, ["let c = duckdb::Connection::open(p);", "let x = 1;"])];
  const plan = buildPlan({
    files: whole,
    config,
    exists: () => false,
    changedSince: new Set([path]),
    delta: [diffOf(path, ["let x = 1;"])]
  });
  assert.deepEqual(plan.checklist, []);
  const full = buildPlan({ files: whole, config, exists: () => false });
  assert.deepEqual(
    full.checklist.map((c) => c.id),
    ["duckdb-open"]
  );
});

test("the general pass is given the triggered items with their sources", () => {
  const plan = buildPlan({
    files: [diffOf("crates/core/src/db.rs", ["let c = duckdb::Connection::open(p);"])],
    config,
    exists: () => false
  });
  const prompt = generalPrompt({
    context: "ctx",
    lenses: [],
    maxFindings: 5,
    checklist: plan.checklist
  });
  assert.ok(prompt.includes("`duckdb-open` (from #2252"));
  assert.ok(prompt.includes("checklist:<id>"));
  assert.equal(checklistBlock([]), "");
  assert.ok(!generalPrompt({ context: "ctx", lenses: [], maxFindings: 5 }).includes("<checklist>"));
});

test("a PR that edits a rule file is judged by the merge base's copy", () => {
  const files = [
    diffOf("crates/app/CLAUDE.md", ["- spawns are fine now"]),
    diffOf("CLAUDE.md", ["x"]),
    diffOf("crates/app/src/server/x.rs", ["let a = 1;"])
  ];
  const plan = buildPlan({ files, config, exists: () => false });
  assert.deepEqual(plan.editedRules, ["crates/app/CLAUDE.md", "CLAUDE.md"]);
  const ctx = (p: typeof plan) =>
    contextBlock({
      repo: "o/r",
      meta: prMeta(),
      plan: p,
      mergeBase: "base123",
      head: "head456",
      prior: [],
      since: null,
      bodyChars: 100
    });
  assert.ok(ctx(plan).includes("`git show base123:<path>`"));
  assert.ok(ctx(plan).includes("- `crates/app/CLAUDE.md`"));
  const code = buildPlan({ files: files.slice(2), config, exists: () => false });
  assert.deepEqual(code.editedRules, []);
  assert.ok(!ctx(code).includes("git show base123"));
});

test("a doc the lenses enforce is a rule file; a doc describing a feature is not", () => {
  const docs = [
    "internal-docs/data-placement.md",
    "internal-docs/customer-apps-functions.md",
    "internal-docs/2026-11-10-org-vs-workspace-grain.md",
    "internal-docs/platform-roadmap.md",
    "crates/app/.claude/skills/oxy-task-spec-default/SKILL.md",
    "product-context.md"
  ];
  const plan = buildPlan({ files: docs.map((p) => diffOf(p, ["x"])), config, exists: () => false });
  assert.deepEqual(plan.editedRules, [
    "internal-docs/data-placement.md",
    "crates/app/.claude/skills/oxy-task-spec-default/SKILL.md",
    "product-context.md"
  ]);
});

test("the summary asks for a step back once reviews in a row keep finding new problems", () => {
  const adding = round({ added: ["F1"], cost: 1 });
  const quiet = round({ added: [], cost: 1 });
  const carried = round({ added: [], cost: 0, mode: "carried" });
  const note = (rounds: ReturnType<typeof round>[]) =>
    convergenceNote(reviewState({ rounds }), config.STEP_BACK_ROUNDS);
  assert.equal(note([adding, adding]), "");
  assert.ok(note([quiet, adding, adding, adding]).includes("The last 3 reviews of this PR"));
  assert.equal(note([adding, quiet, adding, adding]), "", "a quiet review breaks the run");
  assert.equal(note([adding, adding, adding, quiet]), "", "the latest review found nothing new");
  assert.ok(
    note([adding, adding, carried, adding]).includes("The last 3 reviews"),
    "no-model rounds"
  );
  assert.equal(convergenceNote(reviewState({ rounds: [adding, adding, adding] }), undefined), "");
});
