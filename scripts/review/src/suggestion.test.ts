// One-click suggestions, end to end: what a finder returns, what the verifier must vouch
// for, which lines GitHub will take, and what is posted and stored.

import assert from "node:assert/strict";
import { test } from "node:test";
import { hunkText, parseUnifiedDiff } from "./diff.ts";
import { dedupFindings, normalizeFinding } from "./findings.ts";
import { candidate, finding, must, reviewState } from "./fixtures.ts";
import { applyVerification } from "./passes.ts";
import { pickInline, postInline } from "./publish.ts";
import { renderInline } from "./render.ts";
import { addFindings, compactForStorage } from "./state.ts";
import type { GitHub, RawFinding, ReviewState } from "./types.ts";

const raw = (o: RawFinding = {}): RawFinding => ({
  reasoning: "r",
  title: "The owning app is dropped from the share",
  file: "a.ts",
  line: 3,
  category: "correctness",
  severity: "should_fix",
  confidence: 0.9,
  fix: "always include the owner",
  rule: "",
  ...o
});

// Lines 1-4 in one hunk (3 and 4 added); line 20 in a second hunk.
const DIFF = parseUnifiedDiff(`diff --git a/a.ts b/a.ts
--- a/a.ts
+++ b/a.ts
@@ -1,2 +1,4 @@
 function lift() {
   const ids = [...selected];
+  share(ids);
+  remove(owner);
@@ -17,1 +19,2 @@
 }
+export { lift };
`);

test("a suggestion is kept as given: indentation stays, a stray fence and trailing space go", () => {
  const f = normalizeFinding(
    raw({ suggestion: "```ts\n  const ids = [...new Set([...selected, owner])];  \n```" }),
    "general"
  );
  assert.equal(f.suggestion, "  const ids = [...new Set([...selected, owner])];");
  const multi = normalizeFinding(raw({ suggestion: "\n  a();\n  b();\n\n" }), "general");
  assert.equal(multi.suggestion, "  a();\n  b();");
});

test("a suggestion that cannot be posted as given is dropped, not repaired", () => {
  for (const suggestion of ["", "   \n", "a\n```\nb", "x\n".repeat(41), 42, null]) {
    const f = normalizeFinding(raw({ suggestion }), "general");
    assert.equal(f.suggestion, undefined, JSON.stringify(suggestion));
  }
});

test("a merged duplicate never pairs one pass's lines with another's suggestion", () => {
  const withIt = normalizeFinding(raw({ confidence: 0.7, suggestion: "  fixed();" }), "general");
  const surer = normalizeFinding(raw({ line: 4, confidence: 0.9 }), "authz");
  const [m] = dedupFindings([withIt, surer]);
  assert.equal(must(m).line, 4);
  assert.equal(must(m).suggestion, undefined);
  const [kept] = dedupFindings([
    normalizeFinding(raw({ confidence: 0.9, suggestion: "  fixed();" }), "general"),
    normalizeFinding(raw({ confidence: 0.7 }), "authz")
  ]);
  assert.equal(must(kept).suggestion, "  fixed();");
});

test("only a suggestion the verifier vouched for survives", () => {
  const cands = ["C1", "C2", "C3", "C4"].map((cid) =>
    candidate({ cid, title: cid, line: 3, suggestion: "  fixed();" })
  );
  const { confirmed, possible, rejected } = applyVerification(cands, [
    { id: "C1", verdict: "confirmed", severity: "should_fix", reason: "", suggestion_ok: true },
    { id: "C2", verdict: "confirmed", severity: "should_fix", reason: "", suggestion_ok: false },
    { id: "C3", verdict: "confirmed", severity: "should_fix", reason: "" },
    { id: "C4", verdict: "uncertain", severity: "blocking", reason: "maybe" }
  ]);
  assert.deepEqual(
    confirmed.map((c) => [c.cid, c.suggestion]),
    [
      ["C1", "  fixed();"],
      ["C2", undefined],
      ["C3", undefined]
    ]
  );
  assert.equal(must(possible[0]).suggestion, undefined);
  assert.equal(rejected.length, 0);
  const unverified = applyVerification(
    [candidate({ cid: "C1", confidence: 0.9, suggestion: "x" })],
    null
  );
  assert.equal(must(unverified.confirmed[0]).suggestion, undefined, "no verifier, no suggestion");
});

test("hunkText reads head lines only when one hunk holds them all", () => {
  const file = must(DIFF[0]);
  assert.equal(hunkText(file, 3, 4), "  share(ids);\n  remove(owner);");
  assert.equal(hunkText(file, 2, 2), "  const ids = [...selected];");
  assert.equal(hunkText(file, 4, 20), null, "spans two hunks");
  assert.equal(hunkText(file, 5, 5), null, "not in the diff");
  assert.equal(hunkText(file, 4, 3), null);
});

test("a suggestion is posted across exactly the lines it replaces, or not at all", () => {
  const picks = pickInline(
    [
      finding({
        file: "a.ts",
        id: "F1",
        line: 3,
        end_line: 4,
        suggestion: "  share([...ids, owner]);"
      }),
      finding({ file: "a.ts", id: "F2", line: 20, suggestion: "export { lift as liftSecret };" }),
      finding({ file: "a.ts", id: "F3", line: 4, end_line: 20, suggestion: "x" }),
      finding({ file: "a.ts", id: "F4", line: 2, suggestion: "  const ids = [...selected];" }),
      finding({ file: "a.ts", id: "F5", line: 3, severity: "nit", suggestion: "x" })
    ],
    DIFF,
    8
  );
  assert.deepEqual(
    picks.map((p) => [p.finding.id, p.startLine, p.line, p.suggest]),
    [
      ["F1", 3, 4, true],
      ["F2", undefined, 20, true],
      ["F3", undefined, 4, false],
      ["F4", undefined, 2, false]
    ],
    "F3 spans two hunks and F4 suggests what is already there: both go out as words; a nit stays in the summary"
  );
});

test("the inline comment carries the suggestion block only when the pick spans its lines", () => {
  const f = finding({
    file: "a.ts",
    id: "F1",
    line: 3,
    end_line: 4,
    suggestion: "  share([...ids, owner]);"
  });
  const withIt = renderInline(f, { suggest: true });
  assert.match(withIt, /```suggestion\n {2}share\(\[\.\.\.ids, owner\]\);\n```/);
  assert.match(withIt, /Commit the suggestion/);
  assert.match(withIt, /<!-- oxy-review:finding F1 -->$/);
  const without = renderInline(f);
  assert.doesNotMatch(without, /suggestion/);
  assert.doesNotMatch(`${withIt}${without}`, /👍|👎|[Rr]ate it/);
});

test("a multi-line suggestion is sent with start_line on the right side", async () => {
  const sent: Record<string, unknown>[] = [];
  const gh: Pick<GitHub, "createReview" | "listReviewComments"> = {
    createReview: async (_repo, _n, review) => {
      sent.push(review);
      return { id: 7 };
    },
    listReviewComments: async () => [
      { id: 70, pull_request_review_id: 7, body: "<!-- oxy-review:finding F1 -->", user: null }
    ]
  };
  const f = finding({
    file: "a.ts",
    id: "F1",
    line: 3,
    end_line: 4,
    suggestion: "  share([...ids, owner]);"
  });
  const out = await postInline({
    gh,
    repo: "o/r",
    pr: 1,
    head: "abc",
    picks: pickInline([f], DIFF, 8),
    summaryUrl: undefined
  });
  assert.equal(out.get("F1"), 70);
  const [comment] = (sent[0]?.comments ?? []) as Record<string, unknown>[];
  assert.deepEqual(
    [comment?.start_line, comment?.start_side, comment?.line, comment?.side],
    [3, "RIGHT", 4, "RIGHT"]
  );
  assert.match(String(comment?.body), /```suggestion/);
});

test("the stored state never carries a suggestion", () => {
  const { findings } = addFindings(
    reviewState(),
    [candidate({ suggestion: "  fixed();", reasoning: "why", fix: "how" })],
    "abcdef123"
  );
  assert.equal(must(findings[0]).suggestion, "  fixed();", "the round that posts it has it");
  const stored: ReviewState = compactForStorage(reviewState({ findings }));
  assert.equal(must(stored.findings[0]).suggestion, undefined);
  assert.equal(must(stored.findings[0]).fix, "how");
});
