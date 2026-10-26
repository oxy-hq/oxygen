import assert from "node:assert/strict";
import { test } from "node:test";
import * as config from "../config.ts";
import { parseUnifiedDiff } from "./diff.ts";
import { passResult, prMeta, finding as storedFinding } from "./fixtures.ts";
import { buildPlan } from "./plan.ts";
import type { RoundInput } from "./round.ts";
import { runRound } from "./round.ts";
import { emptyState } from "./state.ts";
import type { PassResult, RawFinding, ReviewState, ReviewStatus, RunPass } from "./types.ts";

const files = parseUnifiedDiff(`diff --git a/crates/authz/src/lib.rs b/crates/authz/src/lib.rs
--- a/crates/authz/src/lib.rs
+++ b/crates/authz/src/lib.rs
@@ -1,1 +1,3 @@
 a
+b
+c
`);

const finding = (o: RawFinding): RawFinding => ({
  reasoning: "r",
  title: "t",
  file: "crates/authz/src/lib.rs",
  line: 2,
  category: "authz",
  severity: "should_fix",
  confidence: 0.9,
  fix: "f",
  rule: "",
  ...o
});

/** A pass's output, or a function of its prompt; a pass with none fails on budget. */
type Outputs = Record<string, unknown>;

function fakeLlm(outputs: Outputs) {
  const calls: { name: string; budgetUsd: number; prompt: string }[] = [];
  const runPass: RunPass = async <T>({
    name,
    budgetUsd,
    prompt
  }: Parameters<RunPass>[0]): Promise<PassResult<T>> => {
    calls.push({ name, budgetUsd, prompt });
    const given = outputs[name];
    const output = (
      typeof given === "function" ? (given as (p: string) => unknown)(prompt) : given
    ) as T | undefined;
    return passResult<T>({
      name,
      ok: Boolean(output),
      subtype: output ? "success" : "error_max_budget_usd",
      output: output ?? null,
      cost: 0.4,
      ms: 10,
      turns: 3,
      tools: { Read: 2 }
    });
  };
  return { runPass, calls };
}

function load(prev: ReviewState, mode = "full"): RoundInput {
  const plan = buildPlan({
    files,
    config,
    exists: () => false,
    changedSince: mode === "incremental" ? new Set(["crates/authz/src/lib.rs"]) : null
  });
  return {
    meta: prMeta({ title: "t", body: "" }),
    head: "head2222222",
    mergeBase: "base111",
    plan,
    prev,
    reviewComments: [],
    since: mode === "incremental" ? prev.head : null
  };
}

test("a full round: general + lens find, verifier confirms one and rejects one", async () => {
  const { runPass, calls } = fakeLlm({
    general: {
      findings: [
        finding({ title: "Hand-rolled role check" }),
        finding({ title: "Unrelated", line: 3, category: "correctness" })
      ],
      not_reviewed: []
    },
    authz: {
      findings: [finding({ title: "Hand rolled role check bypasses guard", severity: "blocking" })],
      not_reviewed: []
    },
    verify: (prompt: string) => ({
      results: [...prompt.matchAll(/"id": "(C\d+)"/g)].map(([, id], i) => ({
        id,
        reason: "x",
        verdict: i === 0 ? "confirmed" : "rejected",
        severity: i === 0 ? "blocking" : "nit"
      }))
    })
  });
  const out = await runRound({
    load: load(emptyState(1, config.STATE_VERSION)),
    repo: "o/r",
    runPass,
    cwd: ".",
    graph: false
  });
  assert.deepEqual(
    calls.map((c) => c.name),
    ["general", "authz", "verify"]
  );
  assert.equal(out.state.status, "complete");
  assert.equal(out.state.verdict, "Blocking");
  assert.deepEqual(
    out.state.findings.map((f) => [f.id, f.severity, f.sources]),
    [["F1", "blocking", ["general", "authz"]]]
  );
  assert.equal(out.state.rounds[0]?.cost, 1.2);
  assert.equal(out.state.rounds[0]?.rejected, 1);
  assert.equal(out.view.dropped?.length, 1);
});

test("an incremental round rechecks open findings and closes the fixed one", async () => {
  const prev: ReviewState = {
    ...emptyState(1, config.STATE_VERSION),
    head: "head1111111",
    status: "complete",
    verdict: "Needs changes",
    next: 2,
    findings: [
      storedFinding({
        title: "t",
        file: "crates/authz/src/lib.rs",
        line: 2,
        severity: "should_fix",
        category: "authz",
        reasoning: "r"
      })
    ]
  };
  const { runPass, calls } = fakeLlm({
    general: { findings: [], not_reviewed: [] },
    authz: { findings: [], not_reviewed: [] },
    recheck: { results: [{ id: "F1", status: "fixed", reason: "guard added" }] }
  });
  const out = await runRound({
    load: load(prev, "incremental"),
    repo: "o/r",
    runPass,
    cwd: ".",
    graph: false
  });
  assert.ok(
    calls.some((c) => c.name === "recheck" && c.prompt.includes("git diff head1111111 head2222222"))
  );
  assert.ok(!calls.some((c) => c.name === "verify"), "nothing new to verify");
  assert.equal(out.state.verdict, "LGTM");
  assert.deepEqual(
    out.closedNow.map((f) => [f.id, f.status, f.closed]),
    [["F1", "fixed", "head22222"]]
  );
});

test("a general pass that fails makes the round incomplete; a failed lens only partial", async () => {
  const failGeneral = fakeLlm({ authz: { findings: [], not_reviewed: [] } });
  const a = await runRound({
    load: load(emptyState(1, config.STATE_VERSION)),
    repo: "o/r",
    runPass: failGeneral.runPass,
    cwd: ".",
    graph: false
  });
  assert.equal(a.state.status, "incomplete");
  assert.match(a.view.reason ?? "", /general pass did not finish/);
  const failLens = fakeLlm({ general: { findings: [], not_reviewed: ["web-app"] } });
  const b = await runRound({
    load: load(emptyState(1, config.STATE_VERSION)),
    repo: "o/r",
    runPass: failLens.runPass,
    cwd: ".",
    graph: false
  });
  assert.equal(b.state.status, "partial");
  assert.equal(b.state.verdict, "LGTM");
  assert.deepEqual(b.view.notReviewed, ["web-app (general)"]);
});

test("after a force-push the full review still rechecks open findings against the whole PR", async () => {
  const prev: ReviewState = {
    ...emptyState(1, config.STATE_VERSION),
    head: "rewritten",
    status: "complete",
    verdict: "Blocking",
    next: 2,
    findings: [
      storedFinding({
        title: "t",
        file: "crates/authz/src/lib.rs",
        line: 2,
        severity: "blocking",
        category: "authz",
        reasoning: "r"
      })
    ]
  };
  const { runPass, calls } = fakeLlm({
    general: { findings: [], not_reviewed: [] },
    authz: { findings: [], not_reviewed: [] },
    recheck: { results: [{ id: "F1", status: "fixed", reason: "guard added" }] }
  });
  const out = await runRound({
    load: load(prev, "full"),
    repo: "o/r",
    runPass,
    cwd: ".",
    graph: false
  });
  const recheck = calls.find((c) => c.name === "recheck");
  assert.ok(recheck, "recheck runs in full mode too");
  assert.match(recheck.prompt, /branch was rewritten/);
  assert.match(recheck.prompt, /git diff base111 head2222222/);
  assert.equal(out.state.verdict, "LGTM");
});

test("a skipped, incomplete or unread round is never the base of an incremental review", async () => {
  const { changedSinceLast } = await import("./load.ts");
  const files = [{ path: "a.rs" }];
  const yes = async () => true;
  const statuses: ReviewStatus[] = ["skipped", "incomplete", "none"];
  for (const status of statuses) {
    assert.equal(
      await changedSinceLast({ head: "old", status }, "new", files, ".", yes),
      null,
      status
    );
  }
});

test("the general pass answers the roadmap question; the round keeps it for this head only", async () => {
  const roadmapDoc = `# Platform roadmap

## 2. By audience

### 2.1 Customers use apps

- **Now**
  - **Open apps pick up a new version.** Tabs notice ([#3620]). <!-- rm:open-apps -->
- **Next**
- **Later**
`;
  const { runPass, calls } = fakeLlm({
    general: {
      findings: [],
      not_reviewed: [],
      roadmap: { reasoning: "adds GET /api/x", behaviour: "changed", line: "open-apps" }
    },
    authz: { findings: [], not_reviewed: [] }
  });
  const out = await runRound({
    load: { ...load(emptyState(1, config.STATE_VERSION)), roadmapDoc },
    repo: "o/r",
    runPass,
    cwd: ".",
    graph: false
  });
  const general = calls.find((c) => c.name === "general");
  assert.match(
    general?.prompt ?? "",
    /<roadmap_lines>\n- open-apps \(customers, now\): Open apps pick up a new version\./
  );
  assert.deepEqual(out.state.roadmap, {
    behaviour: "changed",
    reason: "adds GET /api/x",
    line: "open-apps"
  });

  // The next round's general pass fails: the old answer does not speak for the new head.
  const failed = fakeLlm({ authz: { findings: [], not_reviewed: [] } });
  const next = await runRound({
    load: load({ ...out.state, head: "head1111111" }),
    repo: "o/r",
    runPass: failed.runPass,
    cwd: ".",
    graph: false
  });
  assert.equal(next.state.status, "incomplete");
  assert.equal(next.state.roadmap, undefined);
  // No doc on the checkout: the prompt says so instead of listing nothing.
  const noDoc = failed.calls.find((c) => c.name === "general");
  assert.match(noDoc?.prompt ?? "", /The roadmap doc was not readable on this run/);
});

test("an incremental round can raise the roadmap judgement to changed, never lower it", async () => {
  const changed = { behaviour: "changed" as const, reason: "adds a page", line: "new" };
  const prev: ReviewState = {
    ...emptyState(1, config.STATE_VERSION),
    head: "head1111111",
    status: "complete",
    verdict: "LGTM",
    roadmap: changed
  };
  const says = (behaviour: string) =>
    fakeLlm({
      general: {
        findings: [],
        not_reviewed: [],
        roadmap: { reasoning: "only a test in this push", behaviour, line: "" }
      },
      authz: { findings: [], not_reviewed: [] }
    });
  const kept = await runRound({
    load: load(prev, "incremental"),
    repo: "o/r",
    runPass: says("unchanged").runPass,
    cwd: ".",
    graph: false
  });
  assert.deepEqual(kept.state.roadmap, changed);
  // A full review re-reads the whole PR: its answer replaces the old one.
  const full = await runRound({
    load: load(prev),
    repo: "o/r",
    runPass: says("unchanged").runPass,
    cwd: ".",
    graph: false
  });
  assert.equal(full.state.roadmap?.behaviour, "unchanged");
  // And an incremental round that finds behaviour raises an earlier "unchanged".
  const raised = await runRound({
    load: load(
      { ...prev, roadmap: { behaviour: "unchanged", reason: "a fix", line: null } },
      "incremental"
    ),
    repo: "o/r",
    runPass: says("changed").runPass,
    cwd: ".",
    graph: false
  });
  assert.equal(raised.state.roadmap?.behaviour, "changed");
});
