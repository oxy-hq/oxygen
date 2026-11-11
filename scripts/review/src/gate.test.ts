import assert from "node:assert/strict";
import { test } from "node:test";
import { HIGH_RISK, OVERRIDE_LABEL, RELEASE_AUTOMATION, REVIEWER_LOGINS } from "../config.ts";
import { ownersFor, parseCodeowners } from "./codeowners.ts";
import { collectFeedback, discussion, parseFeedback, threadReplies } from "./feedback.ts";
import { finding, must, prMeta, reviewState } from "./fixtures.ts";
import type { GateInput } from "./gate.ts";
import { evaluateGate, evaluateRisk, riskReport, touchedAreas } from "./gate.ts";
import type {
  DiffFile,
  GitHub,
  IssueComment,
  PrMeta,
  PrReview,
  ReviewComment,
  ReviewState
} from "./types.ts";

const state = (o: Partial<ReviewState> = {}): ReviewState =>
  reviewState({ head: "abc", verdict: "LGTM", ...o });
const gate = (s: ReviewState | null, labels: string[] = [], extra: Partial<GateInput> = {}) =>
  evaluateGate({
    state: s,
    headSha: "abc",
    labels,
    draft: false,
    overrideLabel: OVERRIDE_LABEL,
    ...extra
  });

test("review-gate on the release app's PR, which gets no review run of its own", () => {
  assert.equal(gate(null, [], { release: "bump" }).state, "success");
  assert.equal(
    gate(null, [], { release: "other" }).state,
    "failure",
    "a non-bump with no review coming must not wait forever"
  );
  assert.equal(
    gate(state(), [], { release: "other" }).state,
    "success",
    "once a person's push or reopen got it reviewed, the verdict decides"
  );
  assert.equal(gate(null, [OVERRIDE_LABEL], { release: "other" }).state, "success");
});

test("review-gate", () => {
  assert.equal(gate(state()).state, "success");
  assert.equal(
    gate(state({ head: "old" })).state,
    "pending",
    "a review of an older push does not count"
  );
  assert.equal(gate(null).state, "pending");
  assert.equal(gate(state({ status: "incomplete" })).state, "failure");
  const blocking = state({
    verdict: "Blocking",
    findings: [finding({ id: "F3", severity: "blocking" })]
  });
  assert.equal(gate(blocking).state, "failure");
  assert.match(gate(blocking).description, /F3/);
  assert.equal(gate(blocking, [OVERRIDE_LABEL]).state, "success");
  assert.equal(gate(state({ status: "incomplete" }), [OVERRIDE_LABEL]).state, "success");
  assert.equal(gate(state({ verdict: "Needs changes" })).state, "success", "only Blocking gates");
  assert.equal(gate(null, [], { draft: true }).state, "success");
  assert.ok(gate(blocking).description.length <= 140);
  assert.equal(
    gate(null, [], { reviewJobFailed: true }).state,
    "failure",
    "a review that died before posting is not 'waiting'"
  );
  assert.equal(
    gate(state(), [], { reviewJobFailed: true }).state,
    "success",
    "a posted state for the head wins"
  );
});

const OWNERS = `
# comment
/crates/authz/   @alice @bob
/crates/migration/ @carol
/.github/  @alice   # trailing comment
/crates/billing/ @oxy-hq/billing-team
`;

test("CODEOWNERS: last match wins, comments stripped", () => {
  const rules = parseCodeowners(`${OWNERS}\n/crates/authz/src/special.rs @dave`);
  assert.deepEqual(ownersFor("crates/authz/src/lib.rs", rules), ["alice", "bob"]);
  assert.deepEqual(ownersFor("crates/authz/src/special.rs", rules), ["dave"]);
  assert.deepEqual(ownersFor(".github/workflows/x.yaml", rules), ["alice"]);
  assert.deepEqual(ownersFor("README.md", rules), []);
});

test("high-risk approval needs a non-author owner of each risky area", () => {
  const rules = parseCodeowners(OWNERS);
  const review = (login: string, s: string): PrReview => ({ user: { login }, state: s });
  const risk = (files: string[], reviews: PrReview[], author = "alice") =>
    evaluateRisk({ files, reviews, author, rules, highRisk: HIGH_RISK });
  assert.equal(risk(["web-app/src/x.ts"], []).state, "success");
  assert.equal(risk(["crates/authz/src/lib.rs"], []).state, "failure");
  assert.equal(
    risk(["crates/authz/src/lib.rs"], [review("alice", "APPROVED")]).state,
    "failure",
    "authors cannot approve themselves"
  );
  assert.equal(risk(["crates/authz/src/lib.rs"], [review("bob", "APPROVED")]).state, "success");
  assert.equal(
    risk(["crates/authz/src/lib.rs"], [review("bob", "APPROVED"), review("bob", "DISMISSED")])
      .state,
    "failure",
    "the latest review counts"
  );
  assert.equal(
    risk(["crates/authz/src/lib.rs"], [review("bob", "APPROVED"), review("bob", "COMMENTED")])
      .state,
    "success",
    "a later comment keeps the approval"
  );
  const both = risk(
    ["crates/authz/src/a.rs", "crates/migration/src/m.rs"],
    [review("bob", "APPROVED")]
  );
  assert.equal(both.state, "failure");
  assert.match(both.description, /^Needs an approval: migrations, .+ \(carol\)$/);
  assert.equal(
    risk(["crates/billing/src/x.rs"], [review("zed", "APPROVED")]).state,
    "success",
    "team-only ownership accepts any non-author approval"
  );
});

test("every high-risk area says why it needs a person, in a few words", () => {
  for (const area of HIGH_RISK) {
    assert.ok(area.reason.trim().length > 0, area.id);
    assert.ok(area.reason.length <= 45, `${area.id}: keep the reason short`);
  }
});

test("the failing status carries each reason when it fits, and drops them when not", () => {
  const rules = parseCodeowners(OWNERS);
  const risk = (files: string[]) =>
    evaluateRisk({ files, reviews: [], author: "zed", rules, highRisk: HIGH_RISK }).description;
  const authz = must(HIGH_RISK.find((a) => a.id === "authz"));
  assert.equal(
    risk(["crates/authz/src/a.rs"]),
    `Needs an approval: authz, ${authz.reason} (alice or bob)`
  );
  const all = risk([
    "crates/authz/src/a.rs",
    "crates/migration/src/m.rs",
    "crates/billing/src/b.rs",
    ".github/workflows/x.yaml",
    "scripts/review/gate.ts"
  ]);
  assert.ok(all.length <= 140);
  assert.ok(!all.includes(authz.reason), "too long with reasons, so ids and owners only");
  assert.match(all, /^Needs an approval: authz \(alice or bob\); migrations \(carol\)/);
});

test("a touched high-risk area carries its reason and the files that put it there", () => {
  const touched = touchedAreas(
    [
      "web-app/src/x.ts",
      "crates/migration/src/m.rs",
      "crates/authz/src/a.rs",
      "crates/authz/src/a.rs"
    ],
    HIGH_RISK
  );
  assert.deepEqual(
    touched.map((a) => [a.id, a.files]),
    [
      ["authz", ["crates/authz/src/a.rs"]],
      ["migrations", ["crates/migration/src/m.rs"]]
    ],
    "config order, each file once"
  );
  assert.equal(touched[0]?.reason, HIGH_RISK.find((a) => a.id === "authz")?.reason);
  assert.deepEqual(touchedAreas(["web-app/src/x.ts"], HIGH_RISK), []);
});

test("the gate's report says, per area, why, which files and who approved or can", () => {
  const rules = parseCodeowners(OWNERS);
  const risk = evaluateRisk({
    files: ["crates/authz/src/a.rs", "crates/migration/src/m.rs"],
    reviews: [{ user: { login: "bob" }, state: "APPROVED" }],
    author: "alice",
    rules,
    highRisk: HIGH_RISK
  });
  assert.deepEqual(
    risk.areas.map((a) => [a.id, a.owners, a.approvedBy]),
    [
      ["authz", ["alice", "bob"], "bob"],
      ["migrations", ["carol"], null]
    ]
  );
  const report = riskReport(risk.areas);
  const [authz, migrations] = risk.areas.map((a) => a.reason);
  assert.equal(
    report,
    [
      "### high-risk-approval",
      "",
      `- **authz** (${authz}): approved by bob · \`crates/authz/src/a.rs\``,
      `- **migrations** (${migrations}): needs carol · \`crates/migration/src/m.rs\``,
      ""
    ].join("\n")
  );
  assert.doesNotMatch(report, /@/, "a re-run of the gate must not ping owners");
  const many = Array.from({ length: 9 }, (_, i) => `crates/billing/src/f${i}.rs`);
  const billing = evaluateRisk({
    files: many,
    reviews: [],
    author: "a",
    rules,
    highRisk: HIGH_RISK
  });
  assert.match(riskReport(billing.areas), /and 4 more$/m);
});

test("feedback parsing", () => {
  assert.deepEqual(parseFeedback("F3 not useful: the caller checks it"), [
    { id: "F3", kind: "not_useful", note: "not useful: the caller checks it" }
  ]);
  assert.deepEqual(
    parseFeedback("F1, F2 good catch").map((f) => f.id),
    ["F1", "F2"]
  );
  assert.deepEqual(parseFeedback("> F3 not useful\nthanks"), [], "quoted lines are someone else's");
  assert.deepEqual(parseFeedback("see F4"), [], "a mention alone is not a rating");
});

test("bots are first-class: their ratings, replies and disputes count; only the reviewer's own comments do not", () => {
  const findings = [
    { id: "F1", inline: 100 },
    { id: "F2", inline: null }
  ];
  const human = { login: "dev", type: "User" };
  const agent = { login: "claude[bot]", type: "Bot" };
  const reviewer = { login: "github-actions[bot]", type: "Bot" };
  const issueComments: IssueComment[] = [
    { user: human, body: "F2 not useful" },
    { user: agent, body: "F1 not useful: the caller already checks it" },
    { user: reviewer, body: "<!-- oxy-review:summary -->\nF1 not useful (quoted in the summary)" },
    { user: reviewer, body: "Agentic healing: F1 not useful, recorded by another workflow" }
  ];
  const reviewComments: ReviewComment[] = [
    {
      id: 100,
      user: reviewer,
      body: "F1\n<!-- oxy-review:finding F1 -->",
      reactions: { "+1": 2, "-1": 1 }
    },
    { id: 101, in_reply_to_id: 100, user: human, body: "this is wrong, it is guarded" },
    { id: 102, in_reply_to_id: 100, user: agent, body: "Intentional: see the guard in auth.rs" },
    {
      id: 103,
      in_reply_to_id: 100,
      user: reviewer,
      body: "**F1 fixed** in `abc`\n<!-- oxy-review:reply F1 -->"
    }
  ];
  const tally = collectFeedback({
    issueComments,
    reviewComments,
    findings,
    reviewerLogins: REVIEWER_LOGINS
  });
  // 1 reaction + the agent's comment + the other workflow's comment; the summary is the reviewer's own.
  const f1 = must(tally.get("F1"));
  assert.deepEqual(f1.not_useful, 3);
  assert.equal(f1.useful, 2);
  assert.equal(f1.replies, 2, "a person's and an agent's reply; not the reviewer's");
  assert.equal(tally.get("F2")?.not_useful, 1);
  const replies = threadReplies({
    reviewComments,
    issueComments,
    findings,
    reviewerLogins: REVIEWER_LOGINS
  });
  const r1 = must(replies.get("F1"));
  assert.deepEqual(r1.slice(0, 2), [
    "dev: this is wrong, it is guarded",
    "claude[bot] (bot): Intentional: see the guard in auth.rs"
  ]);
  assert.ok(
    r1.some((r) => r.startsWith("claude[bot] (bot): F1 not useful")),
    "a PR comment naming the id counts"
  );
  assert.ok(!r1.some((r) => r.includes("oxy-review")), "never the reviewer's own words");
  const convo = discussion(issueComments, REVIEWER_LOGINS);
  assert.equal(convo.length, 3);
  assert.ok(convo[1]?.startsWith("claude[bot] (bot):"));
});

test("closed findings get a reply in their thread; resolving is best effort", async () => {
  const { resolveClosed } = await import("./publish.ts");
  const calls: [string, number, string][] = [];
  const gh: Pick<GitHub, "replyToReviewComment" | "listReviewThreads" | "resolveThread"> = {
    replyToReviewComment: async (_r, _n, id, body) => {
      calls.push(["reply", id, body]);
      return { id: 1 };
    },
    listReviewThreads: async () => [{ id: "T1", firstComment: 11, resolved: false }],
    resolveThread: async () => {
      throw new Error("Resource not accessible by integration");
    }
  };
  const closed = [
    finding({ id: "F1", inline: 11, status: "fixed", closed: "abc", why: "guarded" }),
    finding({ id: "F2", inline: null, status: "fixed", closed: "abc" })
  ];
  assert.deepEqual(await resolveClosed({ gh, repo: "o/r", pr: 1, closed }), {
    replied: 1,
    resolved: 0
  });
  assert.deepEqual(calls, [
    ["reply", 11, "**F1 fixed** in `abc`: guarded\n<!-- oxy-review:reply F1 -->"]
  ]);
});

test("a release PR skips review only when it is the release app's version bump", async () => {
  const { isReleaseBump } = await import("./release.ts");
  const { parseUnifiedDiff } = await import("./diff.ts");
  const bump = parseUnifiedDiff(`diff --git a/Cargo.toml b/Cargo.toml
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -3,1 +3,1 @@
-version = "0.5.155"
+version = "0.5.156"
diff --git a/CHANGELOG.md b/CHANGELOG.md
--- a/CHANGELOG.md
+++ b/CHANGELOG.md
@@ -1,0 +1,1 @@
+## 0.5.156
`);
  const sneaky = parseUnifiedDiff(`diff --git a/Cargo.toml b/Cargo.toml
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -3,1 +3,2 @@
 version = "0.5.155"
+evil = { git = "https://example.com/x" }
`);
  const app = prMeta({
    user: { login: "oxyhelper", type: "Bot" },
    head: { sha: "h", ref: "release/0.5.156" }
  });
  const is = (meta: PrMeta, files: DiffFile[]) =>
    isReleaseBump({ meta, files }, RELEASE_AUTOMATION);
  assert.ok(is(app, bump));
  assert.ok(!is(app, sneaky), "a non-version line in Cargo.toml needs a review");
  assert.ok(
    !is(app, [
      ...bump,
      ...parseUnifiedDiff(
        "diff --git a/x.rs b/x.rs\n--- a/x.rs\n+++ b/x.rs\n@@ -1,1 +1,1 @@\n-a\n+b\n"
      )
    ]),
    "a pushed code change needs a review"
  );
  assert.ok(
    !is({ ...app, user: { login: "oxyhelper", type: "User" } }, bump),
    "a person named oxyhelper is not the app"
  );
  assert.ok(!is({ ...app, head: { sha: "h", ref: "feat/x" } }, bump));
  assert.ok(
    !is(
      prMeta({ user: { login: "dev", type: "User" }, head: { sha: "h", ref: "release/1" } }),
      bump
    ),
    "the branch name alone is nothing"
  );
});

test("the gate reads a release bump from the API listing, and only a whole one", async () => {
  const { isReleaseBump, listingAsDiff } = await import("./release.ts");
  const app = prMeta({
    user: { login: "oxyhelper[bot]", type: "Bot" },
    head: { sha: "h", ref: "release/0.5.156" }
  });
  const toml = {
    filename: "Cargo.toml",
    status: "modified",
    patch: '@@ -3,1 +3,1 @@\n-version = "0.5.155"\n+version = "0.5.156"'
  };
  const changelog = {
    filename: "CHANGELOG.md",
    status: "modified",
    patch: "@@ -1,0 +1,1 @@\n+## 0.5.156"
  };
  const files = must(listingAsDiff([toml, changelog]));
  assert.ok(isReleaseBump({ meta: app, files }, RELEASE_AUTOMATION));
  const sneaky = must(
    listingAsDiff([{ ...toml, patch: `${toml.patch}\n+evil = { git = "https://example.com/x" }` }])
  );
  assert.ok(!isReleaseBump({ meta: app, files: sneaky }, RELEASE_AUTOMATION));
  assert.equal(
    listingAsDiff([{ filename: "Cargo.lock", status: "modified" }]),
    null,
    "no patch is not proof of a version-only change"
  );
  assert.equal(listingAsDiff([{ ...toml, status: "added" }]), null);
});
