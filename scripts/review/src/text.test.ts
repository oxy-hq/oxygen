import assert from "node:assert/strict";
import { test } from "node:test";
import { suggestionBlock } from "./render.ts";
import { clip, codeSpans, longestTickRun } from "./text.ts";

test("clip leaves text that fits alone, trimmed", () => {
  assert.equal(clip("  short enough  ", 300), "short enough");
});

test("clip ends at the last whole sentence that fits", () => {
  // The reason #4143's F4 closed with, which a hard slice cut at `"nothing wa`.
  const why =
    "`lift()` now records `createdId` and `shared`, and `reportFailure` reports what actually happened. " +
    "If the share fails it deletes the workspace secret it created, and if that cleanup fails it says so. " +
    'If the final delete fails it explains that the app\'s own copy still wins. The misleading "nothing was changed" fallback is gone.';
  const out = clip(why, 300);
  assert.ok(out.length <= 300);
  assert.ok(out.endsWith("still wins."), out);
});

test("clip falls back to a word boundary, marked, when no sentence ends late enough", () => {
  const out = clip(`One long clause ${"and more words ".repeat(30)}`, 80);
  assert.ok(out.length <= 80, `${out.length}`);
  assert.ok(out.endsWith("words…") || out.endsWith("more…") || out.endsWith("and…"), out);
  assert.doesNotMatch(out, /\s…$/);
});

test("clip does not keep a first sentence under half the room", () => {
  const out = clip(`Short. ${"word ".repeat(60)}`, 100);
  assert.ok(out.length > 50 && out.endsWith("…"), out);
});

// The explanation #4153's own review gave for its F1. Its literal ``` and its
// double-backtick span make the backtick count odd with every span closed, which sent the
// first clip() back to the wrong backtick: it kept 431 characters, ending inside
// `start_line..line`.
const F1_REASONING =
  "The new `renderInline` wraps the model's text as `` `\\`\\`\\`suggestion\\n${f.suggestion}\\n\\`\\`\\`` ``. If the replaced lines contain a ``` line, GitHub closes the suggestion block at that line. This happens in markdown docs (CLAUDE.md, internal-docs/*.md, this PR's own prompts/rules.md) and in Rust doc comments that hold code examples. A one-click commit would then put only the text before that line onto the full `start_line..line` span, deleting the rest of the span. The leftover text would show up as stray prose in the comment. `suggestionSpan` in publish.ts doesn't check for this, and the suggestion is never shown in full anywhere else, so the author may commit the truncated version without noticing.";

test("code spans follow CommonMark: a run closes only on a run of the same length", () => {
  const spans = codeSpans(F1_REASONING).map(([s, e]) => F1_REASONING.slice(s, e));
  assert.deepEqual(spans, [
    "`renderInline`",
    "`` `\\`\\`\\`suggestion\\n${f.suggestion}\\n\\`\\`\\``",
    "`start_line..line`",
    "`suggestionSpan`"
  ]);
  assert.deepEqual(codeSpans("a ``` b"), [], "a run with no closer is literal text");
  assert.deepEqual(codeSpans("``a ` b``"), [[0, 9]], "a shorter run inside does not close it");
});

test("clip keeps every sentence that fits, however the backticks count", () => {
  const out = clip(F1_REASONING, 700);
  assert.ok(out.endsWith("stray prose in the comment."), out.slice(-80));
  assert.equal(out, F1_REASONING.slice(0, out.length));
});

test("a cut never lands inside a code span: it moves back to where the span opens", () => {
  const out = clip(`See ${"x ".repeat(20)}\`OrgSide::candidates_with_a_very_long_name_here\``, 60);
  assert.equal(out, `See ${"x ".repeat(19)}x…`);
  // A sentence end inside a span is not a sentence end.
  const dotted = `${"word ".repeat(12)}\`a. b. c. d\` and then more words to run past the room`;
  assert.doesNotMatch(clip(dotted, 70), /a\.$/);
});

test("a span too long to drop is cut inside and closed with its own run", () => {
  const out = clip(`Use \`\`${"x".repeat(200)}\`\` here`, 60);
  assert.ok(out.length <= 60, `${out.length}`);
  assert.match(out, /^Use ``x+``…$/);
});

test("a suggestion fence is longer than any backtick run in the suggestion", () => {
  assert.equal(longestTickRun("a ``` b ` c"), 3);
  assert.equal(longestTickRun("none"), 0);
  assert.equal(suggestionBlock("  fixed();"), "```suggestion\n  fixed();\n```");
  const doc = "Run it:\n```bash\njust up\n```";
  const block = suggestionBlock(doc);
  assert.ok(block.startsWith("````suggestion\n") && block.endsWith("\n````"), block);
  assert.equal(block.slice("````suggestion\n".length, -"\n````".length), doc, "content intact");
});

test("clip hard-cuts a single unbroken token", () => {
  const out = clip("a".repeat(50), 10);
  assert.equal(out, `${"a".repeat(9)}…`);
});
