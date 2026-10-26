// `internal-docs/platform-roadmap.md`, read and edited by line. The doc is hand-written
// markdown with a fixed shape (§1 scoreboard table, §2.x audiences with Now/Next/Later
// bullets, §3 anti-goals table, PR link definitions at the foot); an edit touches only the
// lines of the item it changes so the diff reads like a person made it.
//
// The same parser, with the same ids, is in the roadmap app (oxy-hq/oxygen-roadmap,
// shared/roadmap-doc.ts). Keep the two in step: the gate here and the board there must
// agree on what a line is and what it is called.

export type Audience = "customers" | "engineers" | "platform";
export type Horizon = "now" | "next" | "later";

export const HORIZONS: readonly Horizon[] = ["now", "next", "later"];

export interface Item {
  /** `<!-- rm:id -->` when the line pins one, else the first words of the line. */
  id: string;
  audience: Audience;
  horizon: Horizon;
  landed: boolean;
  /** The line's markdown, continuation lines joined, pin comment included. */
  text: string;
  /** PR numbers the line cites as `[#n]`. */
  prs: number[];
  /** 0-based, inclusive. */
  lineStart: number;
  lineEnd: number;
}

export interface AudienceSection {
  key: Audience;
  title: string;
  horizons: Record<Horizon, Item[]>;
}

export interface Doc {
  northStar: string;
  scoreboard: string[][];
  audiences: AudienceSection[];
  antiGoals: string[][];
  prLinks: Record<number, string>;
}

const AUDIENCE_HEADING: { key: Audience; test: RegExp }[] = [
  { key: "customers", test: /^### 2\.\d+ Customers use apps/ },
  { key: "engineers", test: /^### 2\.\d+ Engineers and agents ship apps/ },
  { key: "platform", test: /^### 2\.\d+ The platform scales/ }
];
const HORIZON_LINE = /^- \*\*(Now|Next|Later)\*\*\s*$/;
const ITEM_LINE = /^ {2}- (.*)$/;
const CONTINUATION = /^ {4}(\S.*)$/;
const PR_REF = /\[#(\d+)\]/g;
const PR_LINK_DEF = /^\[#(\d+)\]:\s*(\S+)\s*$/;
const PINNED = /<!--\s*rm:([a-z0-9][a-z0-9-]{0,62})\s*-->/i;
const ID_WORDS = 6;
const WRAP_AT = 92;

// ── Ids ──────────────────────────────────────────────────────────────────────

export const isValidId = (s: unknown): s is string =>
  typeof s === "string" && /^[a-z0-9][a-z0-9-]{0,62}$/.test(s);

export function pinnedId(text: string): string | null {
  const m = PINNED.exec(text);
  return m?.[1] ? m[1].toLowerCase() : null;
}

/**
 * The text without any pin comment: every one, until nothing changes, so neither a
 * second pin typed into an edit nor one nested inside another outlives the removal.
 */
export function stripPin(text: string): string {
  const all = new RegExp(PINNED.source, "gi");
  let out = text;
  for (let prev = ""; prev !== out; ) {
    prev = out;
    out = out.replace(all, "");
  }
  return out.replace(/\s+$/, "");
}

/** Markdown reduced to plain words: links keep their label, PR refs and emphasis go. */
export function plainWords(text: string): string {
  return stripPin(text)
    .replace(/^\s*Landed:\s*/i, "")
    .replace(/\[#\d+\]/g, " ")
    .replace(/\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/[*_`]/g, "")
    .replace(/['\u2019]/g, "")
    .replace(/[^A-Za-z0-9\s-]/g, " ")
    .replace(/\s+/g, " ")
    .trim()
    .toLowerCase();
}

const TRAILING_STOP = new Set([
  "a",
  "an",
  "and",
  "the",
  "to",
  "of",
  "in",
  "on",
  "for",
  "from",
  "is",
  "its",
  "or",
  "by",
  "with",
  "at",
  "as",
  "this",
  "that",
  "it",
  "you",
  "are",
  "so",
  "can"
]);

/**
 * `derivedId("**App updates go live safely, and can be undone.** Promoting…")` →
 * `app-updates-go-live-safely`: the first six words of the line's bold headline (or of
 * the line, when it has none), without a dangling "and" or "the".
 */
export function derivedId(text: string): string {
  const body = stripPin(text)
    .replace(/^\s*Landed:\s*/i, "")
    .trim();
  const head = /^\*\*(.+?)\*\*/.exec(body)?.[1] ?? body;
  const words = plainWords(head).split(" ").filter(Boolean).slice(0, ID_WORDS);
  while (words.length > 1 && TRAILING_STOP.has(words.at(-1) ?? "")) words.pop();
  const id = words.join("-").replace(/-+/g, "-").replace(/^-|-$/g, "");
  return id || "untitled";
}

export const itemId = (text: string): string => pinnedId(text) ?? derivedId(text);

// ── Parsing ──────────────────────────────────────────────────────────────────

export function prRefs(text: string): number[] {
  const out: number[] = [];
  for (const m of text.matchAll(PR_REF)) {
    const n = Number(m[1]);
    if (!out.includes(n)) out.push(n);
  }
  return out;
}

function tableAfter(lines: string[], header: RegExp): string[][] {
  const i = lines.findIndex((l) => header.test(l));
  if (i < 0) return [];
  const rows: string[][] = [];
  for (let j = i; j < lines.length && lines[j]?.startsWith("|"); j++) {
    const line = lines[j] ?? "";
    const cells = line
      .slice(1, line.endsWith("|") ? -1 : undefined)
      .split("|")
      .map((c) => c.trim());
    if (!cells.every((c) => /^-+$/.test(c))) rows.push(cells);
  }
  return rows.slice(1);
}

function northStar(lines: string[]): string {
  const i = lines.findIndex((l) => l.startsWith("**Oxygen is where"));
  if (i < 0) return "";
  const out: string[] = [];
  for (let j = i; j < lines.length && (lines[j] ?? "").trim() !== ""; j++)
    out.push((lines[j] ?? "").trim());
  return out.join(" ");
}

function prLinks(lines: string[]): Record<number, string> {
  const out: Record<number, string> = {};
  for (const l of lines) {
    const m = PR_LINK_DEF.exec(l);
    if (m?.[1] && m[2]) out[Number(m[1])] = m[2];
  }
  return out;
}

/** Each audience section: from its heading to the next `##`/`###`. */
function audienceRanges(lines: string[]): { key: Audience; start: number; end: number }[] {
  const out: { key: Audience; start: number; end: number }[] = [];
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i] ?? "";
    const h = AUDIENCE_HEADING.find((a) => a.test.test(line));
    if (!h) continue;
    let end = i + 1;
    while (end < lines.length && !/^##/.test(lines[end] ?? "")) end++;
    out.push({ key: h.key, start: i, end });
  }
  return out;
}

export function parseRoadmap(markdown: string): Doc {
  const lines = markdown.split("\n");
  const audiences: AudienceSection[] = [];
  const seen = new Set<string>();

  for (const range of audienceRanges(lines)) {
    const horizons: Record<Horizon, Item[]> = { now: [], next: [], later: [] };
    let horizon: Horizon | null = null;
    let current: Item | null = null;
    const flush = (): void => {
      if (!current) return;
      current.prs = prRefs(current.text);
      current.landed = /^Landed:/i.test(current.text);
      // Two lines starting with the same six words get a suffix, so an id names one
      // line; the pin comment is the way to make an id stable.
      let id = itemId(current.text);
      let n = 2;
      while (seen.has(id)) id = `${itemId(current.text)}-${n++}`;
      seen.add(id);
      current.id = id;
      horizons[current.horizon].push(current);
      current = null;
    };
    for (let i = range.start + 1; i < range.end; i++) {
      const l = lines[i] ?? "";
      const h = HORIZON_LINE.exec(l);
      if (h?.[1]) {
        flush();
        horizon = h[1].toLowerCase() as Horizon;
        continue;
      }
      if (!horizon) continue;
      const it = ITEM_LINE.exec(l);
      if (it) {
        flush();
        current = {
          id: "",
          audience: range.key,
          horizon,
          landed: false,
          text: (it[1] ?? "").trim(),
          prs: [],
          lineStart: i,
          lineEnd: i
        };
        continue;
      }
      const c = current ? CONTINUATION.exec(l) : null;
      if (c && current) {
        current.text += ` ${(c[1] ?? "").trim()}`;
        current.lineEnd = i;
        continue;
      }
      flush(); // a blank line or prose ends the item
    }
    flush();
    audiences.push({
      key: range.key,
      title: (lines[range.start] ?? "").replace(/^### 2\.\d+\s*/, "").trim(),
      horizons
    });
  }

  return {
    northStar: northStar(lines),
    scoreboard: tableAfter(lines, /^\| Metric \|/),
    audiences,
    antiGoals: tableAfter(lines, /^\| We will not \|/),
    prLinks: prLinks(lines)
  };
}

export const allItems = (doc: Doc): Item[] =>
  doc.audiences.flatMap((a) => HORIZONS.flatMap((h) => a.horizons[h]));

export const findItem = (doc: Doc, id: string): Item | undefined =>
  allItems(doc).find((i) => i.id === id);

/** What a line says, for telling a mechanical edit (pins, wrapping) from a real one. */
const said = (i: Item): string =>
  `${i.audience}/${i.horizon}/${stripPin(i.text).replace(/\s+/g, " ").trim()}`;

/** True when the two docs carry the same lines in the same places: nothing moved. */
export function sameLines(before: Doc, after: Doc): boolean {
  const a = allItems(before).map(said);
  const b = allItems(after).map(said);
  return a.length === b.length && a.every((x, i) => x === b[i]);
}

// ── Editing ──────────────────────────────────────────────────────────────────

/**
 * One item's markdown as the doc writes it: `  - ` then 4-space continuations at ~92
 * columns, and its pin comment, when it has one, alone on the last line so the id
 * survives any rewording and never wraps.
 */
export function wrapItem(text: string): string[] {
  const pin = pinnedId(text);
  const words = stripPin(text).replace(/\s+/g, " ").trim().split(" ");
  const out: string[] = [];
  let line = "  -";
  for (const w of words) {
    const candidate = `${line} ${w}`;
    if (candidate.length > WRAP_AT && line !== "  -") {
      out.push(line);
      line = `    ${w}`;
    } else line = candidate;
  }
  out.push(line);
  if (pin) out.push(`    <!-- rm:${pin} -->`);
  return out;
}

function withPrLinkDefs(lines: string[], repo: string, numbers: number[]): string[] {
  const have = prLinks(lines);
  const missing = numbers.filter((n) => !(n in have));
  if (!missing.length) return lines;
  const defs = lines.map((l, i) => ({ l, i })).filter(({ l }) => PR_LINK_DEF.test(l));
  const all = [...Object.keys(have).map(Number), ...missing].sort((x, y) => x - y);
  const block = all.map((n) => `[#${n}]: ${have[n] ?? `https://github.com/${repo}/pull/${n}`}`);
  const first = defs[0];
  const last = defs.at(-1);
  if (first && last) return [...lines.slice(0, first.i), ...block, ...lines.slice(last.i + 1)];
  const out = [...lines];
  while (out.length && (out.at(-1) ?? "").trim() === "") out.pop();
  return [...out, "", "<!-- PR links -->", ...block, ""];
}

function replaceItem(markdown: string, item: Item, text: string, repo: string): string {
  const lines = markdown.split("\n");
  lines.splice(item.lineStart, item.lineEnd - item.lineStart + 1, ...wrapItem(text));
  return withPrLinkDefs(lines, repo, prRefs(text)).join("\n");
}

/** Every line pinned to its current id on a line of its own, so a rewording later keeps it. */
export function pinAll(markdown: string): string {
  const lines = markdown.split("\n");
  // Back to front, so earlier line numbers stay valid; the text is not rewrapped.
  for (const item of allItems(parseRoadmap(markdown)).reverse()) {
    if (pinnedId(item.text)) continue;
    lines.splice(item.lineEnd + 1, 0, `    <!-- rm:${item.id} -->`);
  }
  return lines.join("\n");
}

/**
 * The line `id` cites PR `pr`: `([#pr])` goes on the end, or into the line's last
 * parenthesised PR list when it has one, and the link definition is added.
 */
export function linkPr(markdown: string, id: string, pr: number, repo: string): string {
  const item = findItem(parseRoadmap(markdown), id);
  if (!item) throw new Error(`no roadmap line with id ${JSON.stringify(id)}`);
  if (item.prs.includes(pr)) return markdown;
  const body = stripPin(item.text);
  const pin = pinnedId(item.text);
  const list = /\(((?:\[#\d+\](?:,\s*)?)+)\)(?!.*\(\[#)/;
  const text = list.test(body)
    ? body.replace(list, (_m, refs: string) => `(${refs}, [#${pr}])`)
    : `${body.replace(/\.?\s*$/, "")} ([#${pr}]).`;
  return replaceItem(markdown, item, pin ? `${text} <!-- rm:${pin} -->` : text, repo);
}

// ── The one-page summary ─────────────────────────────────────────────────────

const HEADLINE = /^\*\*(.+?)\*\*/;
const DATE = /\b\d{4}-\d{2}-\d{2}\b/g;

/** Markdown reduced for the summary: PR refs, pins and the doc's cross-links go. */
export function summaryText(text: string): string {
  return stripPin(text)
    .replace(/\s*\(\[#\d+\](?:,\s*\[#\d+\])*\)/g, "")
    .replace(/\[#\d+\]/g, "")
    .replace(/\(\s*\[([^\]]*)\]\([^)]*\)(?:[^()]*)\)/g, "")
    .replace(/\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/\(\s*\)/g, "")
    .replace(/\s+([,.;:])/g, "$1")
    .replace(/\s+/g, " ")
    .trim();
}

/**
 * A line's one-page form: its bold headline when it opens with one (every line does
 * since #3747), else the whole line reduced; `Done.` after a landed line.
 */
export function headline(text: string): string {
  const body = stripPin(text)
    .replace(/^\s*Landed:\s*/i, "")
    .trim();
  const h = HEADLINE.exec(body);
  const words = h?.[1] ? h[1].trim() : summaryText(body);
  return /^Landed:/i.test(text.trim()) ? `${words} Done.` : words;
}

/** A scoreboard cell for the one-page table: no parentheses, and only up to its first `;`. */
function cellSummary(text: string): string {
  const h = HEADLINE.exec(text.trim());
  if (h?.[1]) return h[1].trim().replace(/\.$/, "");
  return (
    summaryText(text.replace(/\s*\([^()]*\)/g, ""))
      .split(";")[0]
      ?.trim() ?? ""
  );
}

function wrapProse(text: string): string[] {
  const out: string[] = [];
  let line = "";
  for (const w of text.split(/\s+/).filter(Boolean)) {
    if (`${line} ${w}`.trim().length > WRAP_AT && line) {
      out.push(line);
      line = w;
    } else line = line ? `${line} ${w}` : w;
  }
  if (line) out.push(line);
  return out;
}

/** The dates the scoreboard's introduction names, for the summary's own one line about them. */
function readingDates(markdown: string): string[] {
  const lines = markdown.split("\n");
  const table = lines.findIndex((l) => /^\| Metric \|/.test(l));
  if (table < 0) return [];
  let end = table;
  while (end > 0 && (lines[end - 1] ?? "").trim() === "") end--;
  let start = end;
  while (start > 0 && (lines[start - 1] ?? "").trim() !== "") start--;
  const found = new Set<string>();
  for (const l of lines.slice(start, end)) for (const d of l.match(DATE) ?? []) found.add(d);
  return [...found].sort();
}

/**
 * `platform-roadmap-summary.md` from the roadmap: deterministic, so the governance rule
 * that the summary carries no fact the roadmap lacks holds by construction, and the
 * `roadmap` status can check it.
 */
export function renderSummary(markdown: string): string {
  const doc = parseRoadmap(markdown);
  const out: string[] = [
    "# Platform roadmap, on one page",
    "",
    "Regenerated from [platform-roadmap.md](./platform-roadmap.md) by",
    "`node scripts/roadmap/roadmap.ts summary --write`; that file wins on any difference.",
    "Horizons are confidence, not dates: **Now** has work under way, **Next** is designed,",
    "**Later** is a direction.",
    "",
    "## North star",
    "",
    ...wrapProse(doc.northStar),
    ""
  ];
  if (doc.scoreboard.length) {
    const dates = readingDates(markdown);
    const read = dates.length ? `, read on ${dates.join(" and ")}` : "";
    out.push("## Scoreboard", "");
    out.push(
      ...wrapProse(
        `Numbers from the weekly usage report and admin views${read}. Directions, not targets.`
      )
    );
    out.push("", "| Metric | Today | Direction |", "| --- | --- | --- |");
    for (const r of doc.scoreboard)
      out.push(`| ${cellSummary(r[0] ?? "")} | ${cellSummary(r[1] ?? "")} | ${r[2] ?? ""} |`);
    out.push("");
  }
  for (const h of HORIZONS) {
    const sections = doc.audiences.filter((a) => a.horizons[h].length);
    if (!sections.length) continue;
    out.push(`## ${h[0]?.toUpperCase()}${h.slice(1)}`, "");
    for (const a of sections) {
      out.push(`- **${a.title}**`);
      for (const it of a.horizons[h]) out.push(`  - ${headline(it.text)}`);
    }
    out.push("");
  }
  if (doc.antiGoals.length) {
    out.push("## What we will not do", "");
    for (const g of doc.antiGoals) out.push(`- ${cellSummary(g[0] ?? "")}.`);
    out.push("", "Where each was decided is in the roadmap.", "");
  }
  return out.join("\n");
}
