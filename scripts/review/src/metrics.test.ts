import assert from "node:assert/strict";
import { test } from "node:test";
import * as config from "../config.ts";
import type { Intro } from "./escapes.ts";
import { attribute, introducedBy, isFixTitle, parseBlame, wasFlagged } from "./escapes.ts";
import { finding, prMeta, round } from "./fixtures.ts";
import type { PrRecord } from "./metrics.ts";
import { aggregate, prRecord } from "./metrics.ts";
import { renderReport } from "./report.ts";
import type { GoldenCase } from "./score.ts";
import { scoreCase, summarize } from "./score.ts";
import { emptyState, encodeState } from "./state.ts";
import type { PrMeta, ReviewState } from "./types.ts";

const human = { login: "dev", type: "User" };

function v2State(o: Partial<ReviewState> = {}): ReviewState {
  return {
    ...emptyState(1, config.STATE_VERSION),
    head: "h1",
    status: "complete",
    verdict: "Needs changes",
    findings: [
      finding({
        id: "F1",
        title: "scan",
        file: "a.rs",
        line: 10,
        severity: "should_fix",
        category: "performance",
        lens: "general",
        rule: "R-S1",
        status: "fixed"
      }),
      finding({
        id: "F2",
        title: "nit",
        file: "b.rs",
        line: 3,
        severity: "nit",
        category: "maintainability",
        lens: "authz",
        rule: null,
        status: "withdrawn"
      }),
      finding({
        id: "F3",
        title: "leak",
        file: "c.rs",
        line: 7,
        severity: "blocking",
        category: "tenant-isolation",
        lens: "authz",
        rule: "R-S1",
        status: "open"
      })
    ],
    rounds: [
      round({ cost: 1.5, ms: 120_000, candidates: 5, rejected: 2 }),
      round({ cost: 0.5, ms: 40_000, candidates: 1, rejected: 0 })
    ],
    ...o
  };
}

const pr = (o: Partial<PrMeta> = {}): PrMeta =>
  prMeta({
    title: "feat: x",
    merged_at: "2026-10-10T00:00:00Z",
    additions: 900,
    deletions: 200,
    head: { sha: "h1" },
    ...o
  });

test("a v2 PR record carries findings, replies, cost and coverage", () => {
  const comments = [
    {
      user: { login: "github-actions[bot]", type: "Bot" },
      body: `${config.SUMMARY_MARKER}\n${encodeState(v2State())}`
    },
    { user: human, body: "F2 is wrong: the caller checks it" }
  ];
  const r = prRecord({
    pr: pr(),
    comments,
    reviewComments: [],
    reviews: [{ user: { login: "ana" }, state: "APPROVED" }],
    files: ["crates/authz/src/lib.rs"],
    lastCommitAt: null,
    config
  });
  assert.equal(r.review, "v2");
  assert.equal(r.reviewedHead, true);
  assert.equal(r.cost, 2);
  assert.equal(r.rounds, 2);
  assert.equal(r.approved, true);
  assert.deepEqual(r.highRisk, ["authz"]);
  assert.equal(r.findings.find((f) => f.id === "F2")?.replies, 1);
  assert.equal(r.findings.find((f) => f.id === "F1")?.replies, 0);
});

test("legacy and unreviewed PRs are recognised", () => {
  const legacy = prRecord({
    pr: pr({ number: 2 }),
    comments: [
      {
        user: { login: "claude" },
        created_at: "2026-10-09T10:00:00Z",
        body: "**Needs changes**\n- `x.rs:3` bug"
      }
    ],
    reviewComments: [],
    reviews: [],
    files: [],
    lastCommitAt: "2026-10-09T11:00:00Z",
    config
  });
  assert.deepEqual(
    [legacy.review, legacy.verdict, legacy.reviewedHead],
    ["legacy", "Needs changes", false]
  );
  const none = prRecord({
    pr: pr({ number: 3, labels: [{ name: config.OVERRIDE_LABEL }] }),
    comments: [],
    reviewComments: [],
    reviews: [],
    files: [],
    lastCommitAt: null,
    config
  });
  assert.deepEqual([none.review, none.override], ["none", true]);
});

test("aggregate: outcomes, not ratings; addressed means fixed or obsolete; rules on two PRs recur", () => {
  const comments = [
    {
      user: { login: "github-actions[bot]", type: "Bot" },
      body: `${config.SUMMARY_MARKER}\n${encodeState(v2State())}`
    }
  ];
  const a = prRecord({
    pr: pr(),
    comments: [...comments, { user: human, body: "F3: deliberate, see the design doc" }],
    reviewComments: [],
    reviews: [],
    files: [],
    lastCommitAt: null,
    config
  });
  const b = prRecord({
    pr: pr({ number: 9, head: { sha: "other" }, additions: 5, deletions: 0 }),
    comments,
    reviewComments: [],
    reviews: [],
    files: [],
    lastCommitAt: null,
    config
  });
  const agg = aggregate([a, b]);
  assert.equal(agg.findings.n, 6);
  assert.equal(agg.findings.addressedPct, 33);
  assert.equal(agg.findings.withdrawnPct, 33);
  assert.equal(agg.findings.openPct, 33);
  assert.equal(agg.findings.replied, 1, "one finding was answered, on one PR");
  assert.equal(agg.bySeverity.blocking?.replied, 1);
  assert.deepEqual(agg.coverage.mergedUnreviewed, [9]);
  assert.deepEqual(agg.recurring, [{ rule: "R-S1", prs: [1, 9] }]);
  assert.equal(agg.byLens.authz?.n, 4);
  assert.equal(agg.size.over1000, 1);
  const report = renderReport({
    agg,
    escapes: [
      {
        fix: 20,
        fixTitle: "fix: y",
        intro: 1,
        files: ["c.rs"],
        file: "c.rs",
        flagged: true,
        finding: "F3",
        status: "open"
      }
    ],
    window: { from: "a", to: "b" }
  });
  assert.ok(
    report.includes("- 2 PRs merged.") &&
      report.includes("flagged as F3") &&
      report.includes("R-S1")
  );
  assert.match(report, /33% still open at merge\./);
  assert.match(report, /\| open at merge \| replied to \|/);
  assert.doesNotMatch(report, /👍|👎|rated/, "nobody is asked to rate a finding");
});

test("fix titles", () => {
  assert.ok(isFixTitle("fix: a"));
  assert.ok(isFixTitle("fix(web): a"));
  assert.ok(!isFixTitle("feat: fix things"));
});

test("blame porcelain: summary is attached to every line of its commit", () => {
  const sha1 = "a".repeat(40);
  const sha2 = "b".repeat(40);
  const text = [
    `${sha1} 10 3 2`,
    "author x",
    "summary feat: thing (#12)",
    "\tline",
    `${sha1} 11 4`,
    "\tline",
    `${sha2} 5 5 1`,
    "summary fix: other (#13)",
    "\tline"
  ].join("\n");
  assert.deepEqual(parseBlame(text), [
    { sha: sha1, orig: 10, summary: "feat: thing (#12)" },
    { sha: sha1, orig: 11, summary: "feat: thing (#12)" },
    { sha: sha2, orig: 5, summary: "fix: other (#13)" }
  ]);
});

test("introducedBy maps blamed commits to PR numbers and skips the fix itself", async () => {
  const sha = "c".repeat(40);
  const fake = async (args: readonly string[]) => {
    if (args[0] === "diff")
      return "diff --git a/x.rs b/x.rs\n--- a/x.rs\n+++ b/x.rs\n@@ -5,1 +5,1 @@\n-old\n+new\n";
    if (args[0] === "blame") return `${sha} 4 5 1\nsummary feat: add x (#40)\n\told\n`;
    throw new Error(args.join(" "));
  };
  assert.deepEqual(await introducedBy({ git: fake, fix: { sha: "f", number: 50 } }), [
    { pr: 40, file: "x.rs", lines: [4], kind: "changed" }
  ]);
});

test("wasFlagged matches nearby v2 findings and legacy mentions", () => {
  const rec: Pick<PrRecord, "review" | "findings"> = {
    review: "v2",
    findings: [
      {
        id: "F1",
        title: "t",
        file: "x.rs",
        line: 10,
        severity: "nit",
        category: "correctness",
        lens: "general",
        rule: null,
        status: "withdrawn",
        replies: 0,
        suggested: false
      }
    ]
  };
  assert.deepEqual(wasFlagged(rec, "x.rs", [20]), {
    flagged: true,
    finding: "F1",
    status: "withdrawn"
  });
  assert.deepEqual(wasFlagged(rec, "x.rs", [40]), { flagged: false });
  assert.deepEqual(wasFlagged({ review: "legacy", findings: [] }, "src/x.rs", [4]), {
    flagged: null,
    legacy: true
  });
  assert.equal(wasFlagged({ review: "none", findings: [] }, "x.rs", [1]).unreviewed, true);
});

test("eval scoring: found, low severity, possible-only, missed, and clean-case false alarms", () => {
  const kase: GoldenCase = {
    id: "k",
    pr: 1,
    base: "b",
    head: "h",
    expect: [
      { file: "a.rs", line: 10, severity: "should_fix", what: "scan" },
      { file: "b.rs", line: 5, severity: "blocking", what: "leak" },
      { file: "c.rs", line: 1, what: "maybe" },
      { file: "d.rs", line: 1, what: "gone" }
    ]
  };
  const s = scoreCase(kase, {
    findings: [
      { file: "a.rs", line: 14, severity: "should_fix", title: "scan" },
      { file: "b.rs", line: 5, severity: "nit", title: "leak-ish" },
      { file: "z.rs", line: 1, severity: "nit", title: "other" }
    ],
    possible: [{ file: "c.rs", line: 2, title: "maybe" }],
    cost: 1.2
  });
  assert.deepEqual(
    s.hits.map((h) => h.result),
    ["found", "found-low-severity", "possible-only", "missed"]
  );
  assert.equal(s.extra.length, 1);
  const clean = scoreCase(
    { id: "c", pr: 2, base: "b", head: "h", expect: [] },
    {
      findings: [{ file: "a", line: 1, severity: "should_fix", title: "x" }],
      possible: [],
      cost: 1
    }
  );
  assert.equal(clean.cleanFalseAlarms, 1);
  const sum = summarize([s, clean]);
  assert.deepEqual(
    [sum.expected, sum.found, sum.foundLow, sum.recall, sum.cleanFalseAlarms, sum.cost],
    [4, 1, 1, 50, 1, 2.2]
  );
});

test("GraphQL PR nodes become the REST shapes the record reads", async () => {
  const { fromGraphql } = await import("./fetch-merged.ts");
  const node = {
    number: 5,
    title: "fix: x",
    mergedAt: "2026-10-10T00:00:00Z",
    additions: 3,
    deletions: 1,
    headRefOid: "h5",
    baseRefName: "main",
    baseRepository: { defaultBranchRef: { name: "main" } },
    mergeCommit: { oid: "m5" },
    author: { login: "dev", __typename: "User" },
    labels: { nodes: [{ name: config.OVERRIDE_LABEL }] },
    commits: { nodes: [{ commit: { committedDate: "2026-10-09T00:00:00Z" } }] },
    comments: {
      nodes: [{ body: "F1 noted", createdAt: "t", author: { login: "dev", __typename: "User" } }]
    },
    reviews: { nodes: [{ state: "APPROVED", author: { login: "ana", __typename: "User" } }] },
    reviewThreads: {
      nodes: [
        {
          comments: {
            nodes: [
              {
                databaseId: 9,
                body: "b",
                author: { login: "github-actions", __typename: "Bot" },
                replyTo: null
              },
              {
                databaseId: 10,
                body: "no",
                author: { login: "dev", __typename: "User" },
                replyTo: { databaseId: 9 }
              }
            ]
          }
        }
      ]
    }
  };
  const f = fromGraphql(node);
  assert.deepEqual(f.pr.base, { ref: "main", repo: { default_branch: "main" } });
  assert.equal(f.pr.merge_commit_sha, "m5");
  const [top, reply] = f.reviewComments;
  assert.equal(top?.user?.type, "Bot");
  assert.equal(reply?.in_reply_to_id, 9);
  assert.equal(f.lastCommitAt, "2026-10-09T00:00:00Z");
  const r = prRecord({ ...f, files: [], config });
  assert.deepEqual([r.review, r.override, r.approved], ["none", true, true]);
});

test("attribution keeps the PRs that wrote most of what a fix changed, and skips broad fixes", () => {
  const intro = (pr: number, file: string, n: number): Intro => ({
    pr,
    file,
    lines: Array.from({ length: n }, (_, i) => i + 1),
    kind: "changed"
  });
  const { culprits } = attribute([
    intro(1, "a.rs", 8),
    intro(2, "b.rs", 1),
    intro(1, "c.rs", 2),
    intro(3, "d.rs", 5)
  ]);
  assert.deepEqual(
    culprits.map((c) => [c.pr, c.lines, c.files.length]),
    [
      [1, 10, 2],
      [3, 5, 1]
    ]
  );
  assert.deepEqual(attribute([intro(1, "a.rs", 400)]), { broad: true, culprits: [] });
  assert.deepEqual(attribute([]), { broad: false, culprits: [] });
});

test("the report compares findings posted with a one-click suggestion against the rest", () => {
  const state = v2State({
    findings: [
      finding({ id: "F1", severity: "should_fix", status: "fixed", suggested: true }),
      finding({ id: "F2", severity: "should_fix", status: "open", suggested: true }),
      finding({ id: "F3", severity: "blocking", status: "fixed" }),
      finding({ id: "F4", severity: "nit", status: "open" })
    ]
  });
  const rec = prRecord({
    pr: pr(),
    comments: [
      {
        user: { login: "github-actions[bot]", type: "Bot" },
        body: `${config.SUMMARY_MARKER}\n${encodeState(state)}`
      }
    ],
    reviewComments: [],
    reviews: [],
    files: [],
    lastCommitAt: null,
    config
  });
  const agg = aggregate([rec]);
  assert.deepEqual(
    [agg.suggestions.with.n, agg.suggestions.with.addressed, agg.suggestions.without.n],
    [2, 1, 1],
    "nits are never posted inline, so they are not the comparison"
  );
  const report = renderReport({ agg, escapes: [], window: { from: "a", to: "b" } });
  assert.match(
    report,
    /One-click suggestions: 2 posted, 50% addressed; should-fix and blocking findings without one: 100% of 1\./
  );
  const none = renderReport({ agg: aggregate([]), escapes: [], window: { from: "a", to: "b" } });
  assert.match(none, /One-click suggestions: none posted\./);
});
