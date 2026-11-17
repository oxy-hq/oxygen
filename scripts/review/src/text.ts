// Model prose in Markdown: shortening it without cutting a word or a code span in half,
// and fencing it without the content closing the fence.

const SENTENCE_END = /[.!?](?=\s)/g;
const TICKS = /`+/g;

/**
 * The inline code spans of `s`, as [start, end) offsets, by CommonMark's rule: a run of
 * n backticks opens a span that the next run of exactly n closes; a run with no closer
 * is literal text. Counting backticks is not enough: a literal ``` or a double-backtick
 * span makes the count odd with every span closed (what #4153's own review stored).
 */
export function codeSpans(s: string): [number, number][] {
  const runs = [...s.matchAll(TICKS)].map((m) => ({ at: m.index, n: m[0].length }));
  const spans: [number, number][] = [];
  for (let i = 0; i < runs.length; i++) {
    const open = runs[i];
    if (!open) continue;
    const j = runs.findIndex((r, k) => k > i && r.n === open.n);
    if (j < 0) continue;
    const close = runs[j];
    if (!close) continue;
    spans.push([open.at, close.at + close.n]);
    i = j;
  }
  return spans;
}

/** The longest run of backticks in `s`: a fence must be longer for `s` not to close it. */
export const longestTickRun = (s: string): number =>
  Math.max(0, ...[...s.matchAll(TICKS)].map((m) => m[0].length));

/**
 * `text`, at most `max` characters. A longer text ends at the last full sentence that
 * fits; failing that, at the last whole word, marked with "…". A cut never lands inside a
 * code span of the full text: it moves back to where the span opens. A reader sees a
 * finished thought, not `The misleading "nothing wa` (what a hard slice posted on #4143).
 * A cut that would keep under half of `max` falls back to the next rule.
 */
export function clip(text: string, max: number): string {
  const t = text.trim();
  if (t.length <= max) return t;
  const floor = Math.floor(max / 2);
  const spans = codeSpans(t);
  // A cut at `at` keeps t[0, at): it splits a span that opened before `at` and closes after.
  const outside = (at: number): number => spans.find(([s, e]) => s < at && at < e)?.[0] ?? at;
  const ends = [...t.slice(0, max).matchAll(SENTENCE_END)].map((m) => m.index + 1);
  const sentence = ends.filter((at) => outside(at) === at && at >= floor).at(-1);
  if (sentence !== undefined) return t.slice(0, sentence);
  const room = t.slice(0, max - 1);
  const space = room.search(/\s\S*$/);
  const word = space >= floor ? space : room.length;
  const at = outside(word);
  if (at === word || at >= floor) return `${t.slice(0, at).replace(/[\s,;:([-]+$/, "")}…`;
  // A span too long to drop (most of the room): cut inside it and close it with its own run.
  const ticks = t.slice(at).match(/^`+/)?.[0] ?? "`";
  return `${t.slice(0, Math.min(word, max - 1 - ticks.length)).trimEnd()}${ticks}…`;
}
