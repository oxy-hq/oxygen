// Turning raw model output into findings worth posting: normalise, merge, filter, judge.

import type * as Config from "../config.ts";
import { clip } from "./text.ts";
import type { Candidate, Category, Finding, RawFinding, Severity, Verdict } from "./types.ts";
import { CATEGORIES, SEVERITIES } from "./types.ts";

type FilterConfig = Pick<typeof Config, "SUPPRESSED_CATEGORIES" | "MIN_CONFIDENCE" | "MAX_NITS">;

export interface Dropped {
  finding: Candidate;
  reason: string;
}

const RANK: Record<Severity, number> = { blocking: 3, should_fix: 2, nit: 1 };

export const severityRank = (s: Severity): number => RANK[s];

const STOP = new Set([
  "the",
  "a",
  "an",
  "of",
  "in",
  "on",
  "to",
  "is",
  "and",
  "or",
  "for",
  "when",
  "with",
  "this",
  "that",
  "not",
  "be",
  "it",
  "by",
  "at",
  "from",
  "as",
  "are",
  "can"
]);

function words(text: string): Set<string> {
  return new Set(
    text
      .toLowerCase()
      .replace(/[`'"]/g, "")
      .split(/[^a-z0-9_]+/)
      .filter((w) => w.length > 2 && !STOP.has(w))
  );
}

/** Jaccard similarity of two titles' word sets. */
export function similarity(a: string, b: string): number {
  const x = words(a);
  const y = words(b);
  if (!x.size || !y.size) return 0;
  let both = 0;
  for (const w of x) if (y.has(w)) both++;
  return both / (x.size + y.size - both);
}

const text = (v: unknown): string =>
  typeof v === "string" ? v : typeof v === "number" ? String(v) : "";

const cleanPath = (p: unknown): string =>
  text(p)
    .trim()
    .replace(/^`|`$/g, "")
    .replace(/^(\.\/|[ab]\/)/, "")
    .replace(/:\d+(-\d+)?$/, "");

// A suggestion replaces whole lines in one click, so it is posted as given or not at all.
const MAX_SUGGESTION_LINES = 40;

/** The replacement text, or nothing when it cannot be posted as a suggestion as given. */
function suggestionOf(v: unknown): string | undefined {
  if (typeof v !== "string") return undefined;
  let s = v.replace(/\r\n/g, "\n");
  // A model that wrapped it in a fence anyway: keep what is inside.
  const fenced = s.match(/^\s*```[^\n]*\n([\s\S]*?)\n?```\s*$/);
  if (fenced) s = fenced[1] ?? "";
  // Leading blank lines and trailing whitespace go; the first line's indentation stays.
  s = s.replace(/^\n+/, "").replace(/\s+$/, "");
  if (!s.trim() || s.includes("```") || s.split("\n").length > MAX_SUGGESTION_LINES)
    return undefined;
  return s;
}

const isSeverity = (v: unknown): v is Severity => (SEVERITIES as readonly unknown[]).includes(v);
const isCategory = (v: unknown): v is Category => (CATEGORIES as readonly unknown[]).includes(v);

/** Coerce one model finding into the shape the pipeline trusts. */
export function normalizeFinding(raw: RawFinding, source: string): Candidate {
  const line = Math.max(1, Math.trunc(Number(raw.line) || 1));
  const end = Math.trunc(Number(raw.end_line) || 0);
  const rule = text(raw.rule).trim();
  const suggestion = suggestionOf(raw.suggestion);
  return {
    title: clip(text(raw.title), 200),
    file: cleanPath(raw.file),
    line,
    end_line: end > line ? end : null,
    category: isCategory(raw.category) ? raw.category : "correctness",
    severity: isSeverity(raw.severity) ? raw.severity : "nit",
    confidence: Math.min(1, Math.max(0, Number(raw.confidence) || 0)),
    reasoning: clip(text(raw.reasoning), 2500),
    fix: clip(text(raw.fix), 1500),
    rule: rule ? rule.slice(0, 120) : null,
    sources: [source],
    ...(suggestion === undefined ? {} : { suggestion })
  };
}

function sameIssue(a: Candidate, b: Candidate, lineSlack: number, minSimilarity: number): boolean {
  if (a.file !== b.file) return false;
  if (Math.abs(a.line - b.line) > lineSlack) return false;
  return a.category === b.category || similarity(a.title, b.title) >= minSimilarity;
}

/** Merge findings that several passes reported about the same place. */
export function dedupFindings(findings: readonly Candidate[]): Candidate[] {
  const out: Candidate[] = [];
  for (const f of findings) {
    const twin = out.find((o) => sameIssue(o, f, 5, 0.4));
    if (!twin) {
      out.push({ ...f, sources: [...f.sources] });
      continue;
    }
    // Keep the more confident wording, the worse severity, and every pass that saw it.
    const severity =
      severityRank(f.severity) > severityRank(twin.severity) ? f.severity : twin.severity;
    const confidence = Math.max(twin.confidence, f.confidence);
    const sources = [...new Set([...twin.sources, ...f.sources])];
    if (f.confidence > twin.confidence) {
      Object.assign(twin, f);
      // A suggestion is text for its own pass's lines: never pair it with another's.
      if (f.suggestion === undefined) delete twin.suggestion;
    }
    Object.assign(twin, { severity, confidence, sources });
  }
  return out;
}

/** Drop candidates that restate a finding the PR already carries open. */
export function dropKnown(
  candidates: readonly Candidate[],
  openPrior: readonly Pick<Finding, "id" | "file" | "line" | "title">[]
): { kept: Candidate[]; known: Dropped[] } {
  const kept: Candidate[] = [];
  const known: Dropped[] = [];
  for (const c of candidates) {
    const prior = openPrior.find(
      (p) =>
        p.file === c.file && Math.abs(p.line - c.line) <= 20 && similarity(p.title, c.title) >= 0.5
    );
    if (prior) known.push({ finding: c, reason: `restates ${prior.id}` });
    else kept.push(c);
  }
  return { kept, known };
}

/** Suppressed categories, confidence floors and the nit cap. */
export function applyFilters(
  findings: readonly Candidate[],
  config: FilterConfig
): { kept: Candidate[]; dropped: Dropped[] } {
  const kept: Candidate[] = [];
  const dropped: Dropped[] = [];
  for (const f of findings) {
    if (config.SUPPRESSED_CATEGORIES.includes(f.category))
      dropped.push({ finding: f, reason: `category ${f.category} is not posted` });
    else if (f.confidence < config.MIN_CONFIDENCE[f.severity])
      dropped.push({
        finding: f,
        reason: `confidence ${f.confidence} under the ${f.severity} floor`
      });
    else kept.push(f);
  }
  const nits = kept.filter((f) => f.severity === "nit").sort((a, b) => b.confidence - a.confidence);
  const overflow = new Set(nits.slice(config.MAX_NITS));
  for (const f of overflow)
    dropped.push({ finding: f, reason: `more than ${config.MAX_NITS} nits` });
  return { kept: kept.filter((f) => !overflow.has(f)), dropped };
}

export const VERDICTS: readonly Verdict[] = ["LGTM", "LGTM with nits", "Needs changes", "Blocking"];

/** The verdict follows from the worst open finding, never from model wording. */
export function verdictFor(openFindings: readonly Pick<Finding, "severity">[]): Verdict {
  const worst = Math.max(0, ...openFindings.map((f) => severityRank(f.severity)));
  return VERDICTS[worst] ?? "Blocking";
}
